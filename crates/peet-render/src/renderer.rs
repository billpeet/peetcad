//! The viewport renderer.
//!
//! Renders the 3D scene into an offscreen texture (with MSAA and its own depth buffer),
//! which the UI then shows as an image. Keeping the viewport in its own texture decouples
//! it from the UI's render pass, and will let us add GPU picking and ambient occlusion
//! passes later without touching the UI.
//!
//! Depth is **reversed** (near = 1, far = 0) with a 32-bit float buffer, which gives
//! near-uniform precision from millimetre details up to kilometre-scale grids.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use peet_math::{Aabb, DMat4, DVec3};
use wgpu::util::DeviceExt as _;

use crate::camera::{Camera, Projection};
use crate::mesh::{ColorVertex, MeshData, MeshVertex, Overlay};

/// Colour format of the viewport image. The shaders write sRGB-encoded values into it,
/// which is what the UI expects when it samples the texture.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Minimum on-screen spacing of the finest visible grid lines, in pixels.
const GRID_MIN_SPACING_PX: f64 = 12.0;

/// Colours for the viewport. All colours are sRGB with alpha in 0..1 unless stated.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewStyle {
    pub background_top: [f32; 3],
    pub background_bottom: [f32; 3],
    pub grid: [f32; 3],
    pub grid_minor_alpha: f32,
    pub grid_major_alpha: f32,
    pub axis_x: [f32; 4],
    pub axis_y: [f32; 4],
    /// Colour of mesh edge lines (sRGB bytes).
    pub edges: [u8; 4],
    /// Colour blended into selected or hovered objects.
    pub highlight: [f32; 3],
}

impl ViewStyle {
    pub fn dark() -> Self {
        Self {
            background_top: [0.235, 0.255, 0.294],
            background_bottom: [0.102, 0.110, 0.125],
            grid: [0.62, 0.66, 0.72],
            grid_minor_alpha: 0.10,
            grid_major_alpha: 0.28,
            axis_x: [0.86, 0.33, 0.33, 0.85],
            axis_y: [0.44, 0.76, 0.36, 0.85],
            edges: [18, 20, 24, 255],
            highlight: [0.33, 0.62, 1.0],
        }
    }

    pub fn light() -> Self {
        Self {
            background_top: [0.80, 0.84, 0.90],
            background_bottom: [0.975, 0.98, 0.99],
            grid: [0.30, 0.34, 0.40],
            grid_minor_alpha: 0.10,
            grid_major_alpha: 0.26,
            axis_x: [0.84, 0.18, 0.18, 0.8],
            axis_y: [0.22, 0.60, 0.16, 0.8],
            edges: [28, 30, 36, 255],
            highlight: [0.10, 0.45, 0.95],
        }
    }
}

/// Handle to a mesh uploaded to the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u64);

/// One mesh to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct ObjectDraw {
    pub mesh: MeshId,
    /// Object-to-world transform (must be rigid).
    pub transform: DMat4,
    pub show_edges: bool,
    /// How strongly to blend in the highlight colour (0 = none, 1 = fully tinted).
    pub highlight: f32,
}

/// Everything the renderer needs for one frame.
pub struct FrameInput<'a> {
    /// Size of the viewport in physical pixels.
    pub size_px: [u32; 2],
    pub camera: &'a Camera,
    pub style: &'a ViewStyle,
    pub show_grid: bool,
    pub objects: &'a [ObjectDraw],
    pub overlay: &'a Overlay,
    /// Bounds of everything in the scene (objects and overlays). Used to set clip planes.
    pub scene_bounds: Aabb,
}

/// Counters for the performance overlay.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub triangles: usize,
    pub lines: usize,
    pub draw_calls: usize,
}

/// Result of rendering a frame.
pub struct RenderOutput<'a> {
    /// The finished, resolved viewport image.
    pub view: &'a wgpu::TextureView,
    /// True if the output texture was (re)created this frame, so the UI must re-register it.
    pub texture_changed: bool,
    pub stats: RenderStats,
}

/// Size of the globals uniform block; checked against the WGSL declaration in tests.
#[cfg(test)]
pub(crate) const GLOBALS_SIZE: usize = std::mem::size_of::<Globals>();

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    viewport: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    grid_color: [f32; 4],
    grid_params: [f32; 4],
    grid_center: [f32; 4],
    axis_x_color: [f32; 4],
    axis_y_color: [f32; 4],
    line_bias: [f32; 4],
    edge_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
}

struct ObjectBinding {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl ObjectBinding {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, label: &str) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(&ObjectUniform {
                model: glam::Mat4::IDENTITY.to_cols_array_2d(),
                tint: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self { buffer, bind_group }
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    edges: Option<wgpu::Buffer>,
    edge_vertex_count: u32,
    object: ObjectBinding,
}

/// A vertex buffer that grows as needed and is rewritten every frame.
struct DynamicBuffer {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
    label: &'static str,
}

impl DynamicBuffer {
    fn new(label: &'static str) -> Self {
        Self {
            buffer: None,
            capacity: 0,
            label,
        }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let needed = data.len() as u64;
        if needed > self.capacity || self.buffer.is_none() {
            self.capacity = needed.next_power_of_two().max(4096);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, data);
        }
    }
}

struct Targets {
    size: [u32; 2],
    msaa_color: Option<wgpu::TextureView>,
    resolved: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Targets {
    fn new(device: &wgpu::Device, size: [u32; 2], samples: u32) -> Self {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let resolved = texture(
            "viewport_color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa_color = (samples > 1).then(|| {
            texture(
                "viewport_color_msaa",
                COLOR_FORMAT,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth = texture(
            "viewport_depth",
            DEPTH_FORMAT,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        Self {
            size,
            msaa_color,
            resolved,
            depth,
        }
    }
}

pub struct ViewportRenderer {
    samples: u32,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    identity_object: ObjectBinding,
    background_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    edge_pipeline: wgpu::RenderPipeline,
    overlay_tri_pipeline: wgpu::RenderPipeline,
    targets: Option<Targets>,
    meshes: HashMap<MeshId, GpuMesh>,
    next_mesh_id: u64,
    overlay_lines: DynamicBuffer,
    overlay_tris: DynamicBuffer,
}

impl ViewportRenderer {
    /// Creates the renderer. `samples` is the MSAA sample count (1 or 4 are always supported).
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let uniform_layout = |label| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        };
        let globals_layout = uniform_layout("globals_layout");
        let object_layout = uniform_layout("object_layout");

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });
        let identity_object = ObjectBinding::new(device, &object_layout, "identity_object");

        let layout_globals_only = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globals_only"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let layout_with_object = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globals_and_object"),
            bind_group_layouts: &[Some(&globals_layout), Some(&object_layout)],
            immediate_size: 0,
        });

        let shader = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{}\n{}", include_str!("shaders/common.wgsl"), source).into(),
                ),
            })
        };
        let background_shader = shader("background", include_str!("shaders/background.wgsl"));
        let grid_shader = shader("grid", include_str!("shaders/grid.wgsl"));
        let mesh_shader = shader("mesh", include_str!("shaders/mesh.wgsl"));
        let color_shader = shader("color", include_str!("shaders/color.wgsl"));

        let mesh_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4],
        };
        let color_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ColorVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        };

        let pipeline = |desc: PipelineDesc<'_>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(desc.label),
                layout: Some(desc.layout),
                vertex: wgpu::VertexState {
                    module: desc.shader,
                    entry_point: Some(desc.vs),
                    buffers: std::slice::from_ref(&desc.vertex_layout),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: desc.topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(desc.depth_write),
                    depth_compare: Some(desc.depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: desc.shader,
                    entry_point: Some(desc.fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: desc.blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let background_pipeline = pipeline(PipelineDesc {
            label: "background",
            layout: &layout_globals_only,
            shader: &background_shader,
            vs: "vs_background",
            fs: "fs_background",
            vertex_layout: None,
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
            blend: false,
        });
        let grid_pipeline = pipeline(PipelineDesc {
            label: "grid",
            layout: &layout_globals_only,
            shader: &grid_shader,
            vs: "vs_grid",
            fs: "fs_grid",
            vertex_layout: None,
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: true,
        });
        let mesh_pipeline = pipeline(PipelineDesc {
            label: "mesh",
            layout: &layout_with_object,
            shader: &mesh_shader,
            vs: "vs_mesh",
            fs: "fs_mesh",
            vertex_layout: Some(mesh_vertex_layout),
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: false,
        });
        let line_pipeline = pipeline(PipelineDesc {
            label: "lines",
            layout: &layout_with_object,
            shader: &color_shader,
            vs: "vs_line",
            fs: "fs_color",
            vertex_layout: Some(color_vertex_layout.clone()),
            topology: wgpu::PrimitiveTopology::LineList,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: true,
        });
        let edge_pipeline = pipeline(PipelineDesc {
            label: "edges",
            layout: &layout_with_object,
            shader: &color_shader,
            vs: "vs_edge",
            fs: "fs_color",
            vertex_layout: Some(color_vertex_layout.clone()),
            topology: wgpu::PrimitiveTopology::LineList,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: true,
        });
        let overlay_tri_pipeline = pipeline(PipelineDesc {
            label: "overlay_triangles",
            layout: &layout_with_object,
            shader: &color_shader,
            vs: "vs_color",
            fs: "fs_color",
            vertex_layout: Some(color_vertex_layout),
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: true,
        });

        Self {
            samples,
            globals_buffer,
            globals_bind_group,
            object_layout,
            identity_object,
            background_pipeline,
            grid_pipeline,
            mesh_pipeline,
            line_pipeline,
            edge_pipeline,
            overlay_tri_pipeline,
            targets: None,
            meshes: HashMap::new(),
            next_mesh_id: 0,
            overlay_lines: DynamicBuffer::new("overlay_lines"),
            overlay_tris: DynamicBuffer::new("overlay_triangles"),
        }
    }

    /// Uploads a mesh to the GPU. It stays resident until [`Self::remove_mesh`].
    pub fn upload_mesh(&mut self, device: &wgpu::Device, mesh: &MeshData) -> MeshId {
        let id = MeshId(self.next_mesh_id);
        self.next_mesh_id += 1;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // Edges only need positions: their colour comes from the style (see `vs_edge`).
        let edge_vertices: Vec<ColorVertex> = mesh
            .edges
            .iter()
            .map(|p| ColorVertex {
                position: *p,
                color: [255; 4],
            })
            .collect();
        let edges = (!edge_vertices.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh_edges"),
                contents: bytemuck::cast_slice(&edge_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let object = ObjectBinding::new(device, &self.object_layout, "mesh_object");
        self.meshes.insert(
            id,
            GpuMesh {
                vertices,
                indices,
                index_count: mesh.indices.len() as u32,
                edges,
                edge_vertex_count: edge_vertices.len() as u32,
                object,
            },
        );
        id
    }

    pub fn remove_mesh(&mut self, id: MeshId) {
        self.meshes.remove(&id);
    }

    /// Renders one frame into the offscreen viewport texture.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &FrameInput<'_>,
    ) -> RenderOutput<'_> {
        let size = [frame.size_px[0].max(1), frame.size_px[1].max(1)];
        let texture_changed = self.targets.as_ref().is_none_or(|t| t.size != size);
        if texture_changed {
            self.targets = Some(Targets::new(device, size, self.samples));
        }

        let camera = frame.camera;
        let aspect = f64::from(size[0]) / f64::from(size[1]);
        let wpp = camera.world_per_pixel(f64::from(size[1]));

        // Grid placement and level of detail.
        let grid_extent = match camera.projection {
            Projection::Perspective => camera.distance * 8.0,
            Projection::Orthographic => camera.view_height() * aspect.max(1.0) * 4.0,
        };
        let level = (wpp * GRID_MIN_SPACING_PX).log10();
        let level_ceil = level.ceil();
        let grid_spacing = 10f64.powf(level_ceil);
        let grid_blend = level_ceil - level;
        let grid_center = DVec3::new(camera.target.x, camera.target.y, 0.0);

        let mut bounds = frame.scene_bounds;
        if frame.show_grid {
            bounds.extend(grid_center - DVec3::new(grid_extent, grid_extent, 0.0));
            bounds.extend(grid_center + DVec3::new(grid_extent, grid_extent, 0.0));
        }
        let view_proj = camera.view_projection(aspect, &bounds);

        let style = frame.style;
        let v4 = |v: DVec3, w: f64| [v.x as f32, v.y as f32, v.z as f32, w as f32];
        let rgb_a = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a];
        let globals = Globals {
            view_proj: view_proj.as_mat4().to_cols_array_2d(),
            eye: v4(
                camera.eye(),
                if camera.projection == Projection::Orthographic {
                    1.0
                } else {
                    0.0
                },
            ),
            forward: v4(camera.forward(), wpp),
            right: v4(camera.right(), 0.0),
            up: v4(camera.up(), 0.0),
            viewport: [
                size[0] as f32,
                size[1] as f32,
                1.0 / size[0] as f32,
                1.0 / size[1] as f32,
            ],
            bg_top: rgb_a(style.background_top, 1.0),
            bg_bottom: rgb_a(style.background_bottom, 1.0),
            grid_color: rgb_a(style.grid, style.grid_major_alpha),
            grid_params: [
                grid_spacing as f32,
                grid_blend as f32,
                grid_extent as f32,
                style.grid_minor_alpha,
            ],
            grid_center: v4(grid_center, 0.0),
            axis_x_color: style.axis_x,
            axis_y_color: style.axis_y,
            line_bias: [5e-4, (camera.view_height() * 5e-4) as f32, 0.0, 0.0],
            edge_color: style.edges.map(|c| f32::from(c) / 255.0),
        };
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        // Per-object uniforms: transform and highlight tint.
        for draw in frame.objects {
            if let Some(mesh) = self.meshes.get(&draw.mesh) {
                let uniform = ObjectUniform {
                    model: draw.transform.as_mat4().to_cols_array_2d(),
                    tint: rgb_a(style.highlight, draw.highlight.clamp(0.0, 1.0) * 0.55),
                };
                queue.write_buffer(&mesh.object.buffer, 0, bytemuck::bytes_of(&uniform));
            }
        }
        self.overlay_lines
            .write(device, queue, bytemuck::cast_slice(&frame.overlay.lines));
        self.overlay_tris.write(
            device,
            queue,
            bytemuck::cast_slice(&frame.overlay.triangles),
        );

        let targets = self.targets.as_ref().expect("targets created above");
        let (color_view, resolve_target) = match &targets.msaa_color {
            Some(msaa) => (msaa, Some(&targets.resolved)),
            None => (&targets.resolved, None),
        };

        let mut stats = RenderStats::default();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        // The multisampled image is only needed until it is resolved.
                        store: if resolve_target.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        // Reversed depth: 0 is infinitely far away.
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);

            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);
            stats.draw_calls += 1;

            // Opaque shaded meshes.
            pass.set_pipeline(&self.mesh_pipeline);
            for draw in frame.objects {
                let Some(mesh) = self.meshes.get(&draw.mesh) else {
                    continue;
                };
                pass.set_bind_group(1, &mesh.object.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.draw_calls += 1;
                stats.triangles += mesh.index_count as usize / 3;
            }

            // Mesh edges.
            pass.set_pipeline(&self.edge_pipeline);
            for draw in frame.objects.iter().filter(|d| d.show_edges) {
                let Some(mesh) = self.meshes.get(&draw.mesh) else {
                    continue;
                };
                if let Some(edges) = &mesh.edges {
                    pass.set_bind_group(1, &mesh.object.bind_group, &[]);
                    pass.set_vertex_buffer(0, edges.slice(..));
                    pass.draw(0..mesh.edge_vertex_count, 0..1);
                    stats.draw_calls += 1;
                    stats.lines += mesh.edge_vertex_count as usize / 2;
                }
            }

            // Transparent layers, back to front-ish: grid, overlay fills, overlay lines.
            if frame.show_grid {
                pass.set_pipeline(&self.grid_pipeline);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
            }
            pass.set_bind_group(1, &self.identity_object.bind_group, &[]);
            if let (Some(buffer), false) = (
                &self.overlay_tris.buffer,
                frame.overlay.triangles.is_empty(),
            ) {
                pass.set_pipeline(&self.overlay_tri_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..frame.overlay.triangles.len() as u32, 0..1);
                stats.draw_calls += 1;
                stats.triangles += frame.overlay.triangles.len() / 3;
            }
            if let (Some(buffer), false) =
                (&self.overlay_lines.buffer, frame.overlay.lines.is_empty())
            {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..frame.overlay.lines.len() as u32, 0..1);
                stats.draw_calls += 1;
                stats.lines += frame.overlay.lines.len() / 2;
            }
        }
        queue.submit([encoder.finish()]);

        RenderOutput {
            view: &self.targets.as_ref().expect("targets exist").resolved,
            texture_changed,
            stats,
        }
    }
}

struct PipelineDesc<'a> {
    label: &'a str,
    layout: &'a wgpu::PipelineLayout,
    shader: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: &'a str,
    vertex_layout: Option<wgpu::VertexBufferLayout<'a>>,
    topology: wgpu::PrimitiveTopology,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    blend: bool,
}
