//! The PeetCAD application shell, built with `egui`.
//!
//! This crate owns everything the user sees: menus, toolbar, panels, the command palette,
//! and the 3D viewport widget. The same code runs natively and in the browser; the
//! `peetcad` binary crate only handles platform startup.

mod app;
pub mod commands;
mod document;
mod palette;
mod perf;
pub mod settings;
mod view_cube;
mod viewport;

pub use app::{APP_NAME, PeetApp, VERSION};
