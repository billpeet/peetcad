//! The PeetCAD viewport renderer, built on `wgpu`.
//!
//! This crate knows nothing about the UI toolkit: it takes a camera, meshes and overlay
//! geometry and produces a texture. The same code runs on DX12/Vulkan natively and on
//! WebGPU/WebGL2 in the browser, so it sticks to features that WebGL2 supports.

pub mod camera;
pub mod mesh;
mod renderer;

pub use camera::{Camera, CameraAnimation, OrbitStyle, Projection, StandardView};
pub use mesh::{ColorVertex, MeshData, MeshVertex, Overlay};
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
    }
}
