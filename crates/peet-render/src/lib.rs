//! The PeetCAD viewport renderer, built on `wgpu`.
//!
//! This crate knows nothing about the UI toolkit: it takes a camera, meshes and overlay
//! geometry and produces a texture. The same code runs on DX12/Vulkan natively and on
//! WebGPU/WebGL2 in the browser, so it sticks to features that WebGL2 supports.

mod batch;
pub mod camera;
pub mod mesh;
mod pick;
mod renderer;

pub use batch::in_view;
pub use camera::{Camera, CameraAnimation, OrbitStyle, Projection, StandardView};
pub use mesh::{ColorVertex, MeshData, MeshVertex, Overlay};
pub use pick::{MAX_PICK_RADIUS_PX, PickHit, PickRequest, PickResult};
pub use renderer::{
    COLOR_FORMAT, FrameInput, MeshId, ObjectDraw, RenderOutput, RenderStats, ViewStyle,
    ViewportRenderer,
};

/// Re-exported so the application uses the exact `wgpu` version the renderer was built against.
pub use wgpu;

#[cfg(test)]
mod shader_tests {
    /// Parses and validates each shader (as assembled at runtime) with naga, so shader
    /// errors fail `cargo test` instead of crashing the app at startup.
    #[test]
    fn shaders_validate() {
        let common = include_str!("shaders/common.wgsl");
        let shaders = [
            ("background", include_str!("shaders/background.wgsl")),
            ("grid", include_str!("shaders/grid.wgsl")),
            ("mesh", include_str!("shaders/mesh.wgsl")),
            ("color", include_str!("shaders/color.wgsl")),
            ("pick", include_str!("shaders/pick.wgsl")),
        ];
        for (name, source) in shaders {
            let full = format!("{common}\n{source}");
            let module = naga::front::wgsl::parse_str(&full)
                .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&full)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&full)));
        }
    }

    /// Every entry point must translate to GLSL ES 3.00, which is what the WebGL2
    /// fallback runs.
    #[test]
    fn shaders_translate_to_webgl2_glsl() {
        use naga::back::glsl;
        let common = include_str!("shaders/common.wgsl");
        let shaders = [
            ("background", include_str!("shaders/background.wgsl")),
            ("grid", include_str!("shaders/grid.wgsl")),
            ("mesh", include_str!("shaders/mesh.wgsl")),
            ("color", include_str!("shaders/color.wgsl")),
            ("pick", include_str!("shaders/pick.wgsl")),
        ];
        for (name, source) in shaders {
            let full = format!("{common}\n{source}");
            let module = naga::front::wgsl::parse_str(&full).expect("parses");
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .expect("validates");
            let options = glsl::Options {
                version: glsl::Version::new_gles(300),
                ..glsl::Options::default()
            };
            for entry in &module.entry_points {
                let pipeline_options = glsl::PipelineOptions {
                    shader_stage: entry.stage,
                    entry_point: entry.name.clone(),
                    multiview: None,
                };
                let mut out = String::new();
                glsl::Writer::new(
                    &mut out,
                    &module,
                    &info,
                    &options,
                    &pipeline_options,
                    naga::proc::BoundsCheckPolicies::default(),
                )
                .and_then(|mut w| w.write().map(|_| ()))
                .unwrap_or_else(|e| panic!("{name}.wgsl {}: {e}", entry.name));
            }
        }
    }

    /// The Rust `Globals` struct must match the WGSL one byte for byte.
    #[test]
    fn globals_layout_matches_wgsl() {
        let full = format!(
            "{}\n{}",
            include_str!("shaders/common.wgsl"),
            include_str!("shaders/background.wgsl")
        );
        let module = naga::front::wgsl::parse_str(&full).expect("parses");
        let globals = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Globals"))
            .expect("Globals struct");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).expect("layout");
        let wgsl_size = layouter[globals.0].size as usize;
        assert_eq!(wgsl_size, crate::renderer::GLOBALS_SIZE);
        // The instance attributes: 4 columns, a tint and a pick id (padded).
        assert_eq!(std::mem::size_of::<crate::batch::Instance>(), 96);
    }
}
