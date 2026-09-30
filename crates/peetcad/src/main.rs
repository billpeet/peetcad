//! PeetCAD entry points: a native window on desktop, a canvas in the browser.
//!
//! Everything interesting lives in `peet-ui` and below. This file only sets up logging,
//! crash reporting and the window or canvas for each platform.

// No console window for release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use peet_platform::Instant;
use peet_ui::{APP_NAME, PeetApp, VERSION};

#[cfg(not(target_arch = "wasm32"))]
mod icon;

fn install_crash_reporting() {
    peet_platform::crash::install_panic_hook(peet_platform::crash::BuildInfo {
        app_name: APP_NAME,
        version: VERSION,
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let process_start = Instant::now();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    install_crash_reporting();
    log::info!(
        "{APP_NAME} {VERSION} starting on {}",
        peet_platform::target_description()
    );

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("peetcad")
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([640.0, 420.0])
            .with_icon(icon::app_icon()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: native_wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(PeetApp::new(cc, process_start)))),
    )
}

/// GPU setup for native builds.
///
/// On Windows only DX12 is initialised by default: probing Vulkan and OpenGL as well
/// roughly doubles GPU start-up time and DX12 is the platform's native API. Set the
/// `WGPU_BACKEND` environment variable (`vulkan`, `dx12`, `gl`) to override this.
#[cfg(not(target_arch = "wasm32"))]
fn native_wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{SurfaceConfig, WgpuConfiguration, WgpuSetup};
    use eframe::wgpu;

    let mut options = WgpuConfiguration {
        // Lowest input-to-photon latency: direct manipulation should feel immediate.
        surface: SurfaceConfig::LOW_LATENCY,
        ..Default::default()
    };
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        let default_backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::PRIMARY
        };
        setup.instance_descriptor.backends = wgpu::Backends::from_env().unwrap_or(default_backends);
    }
    options
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    use eframe::web_sys;

    let process_start = Instant::now();
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    install_crash_reporting();
    log::info!(
        "{APP_NAME} {VERSION} starting on {}",
        peet_platform::target_description()
    );

    wasm_bindgen_futures::spawn_local(async move {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("running in a browser page");
        let canvas = document
            .get_element_by_id("peetcad_canvas")
            .expect("index.html has a canvas with id peetcad_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("peetcad_canvas is a <canvas>");

        let runner = eframe::WebRunner::new();
        let result = runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| Ok(Box::new(PeetApp::new(cc, process_start)))),
            )
            .await;

        let status = document.get_element_by_id("status");
        match result {
            Ok(()) => {
                if let Some(status) = status {
                    status.remove();
                }
            }
            Err(error) => {
                if let Some(status) = status {
                    status.set_inner_html(
                        "<p>PeetCAD could not start.</p><p>It needs a browser with WebGPU or WebGL2. See the developer console for details.</p>",
                    );
                }
                log::error!("Failed to start: {error:?}");
            }
        }
    });
}
