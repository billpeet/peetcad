# PeetCAD

A lightweight parametric 3D CAD application written in Rust, with sheet metal first.
It runs as a native Windows app and in the browser (WebAssembly), from one codebase.

> **Status:** early development (Phase 1: 2D sketcher).
> See [ROADMAP.md](ROADMAP.md) for the plan.

## Running

You need Rust (stable, installed through [rustup](https://rustup.rs)). The pinned toolchain
in `rust-toolchain.toml` pulls in the WebAssembly target automatically.

### Windows desktop

```sh
cargo run --release
```

GPU selection: PeetCAD uses DirectX 12 on Windows. To try another backend, set
`WGPU_BACKEND=vulkan` (or `gl`). `WGPU_POWER_PREF=low` prefers the integrated GPU.

### Web browser

Install [Trunk](https://trunk-rs.github.io/trunk/), then:

```sh
cd crates/peetcad
trunk serve              # development build at http://127.0.0.1:8080
trunk build --release --cargo-profile release-web   # optimised build into dist/
```

The browser needs WebGPU or WebGL2 (any current browser).

## Controls

| Action | Default (SolidWorks-style) |
|---|---|
| Rotate | Middle drag, or Alt + left drag |
| Pan | Ctrl + middle drag, or Alt + Shift + left drag |
| Zoom | Mouse wheel / pinch (zooms at the cursor), or Shift + middle drag |
| Standard views | `0` isometric, `1` front, `2` top, `3` right, `4` back, `5` bottom, `6` left, or click the view cube |
| Zoom to fit | `F` |
| Perspective / orthographic | `P` |
| Command palette | `Ctrl+K` |
| All shortcuts | `F1` |
| Performance overlay | `F3` |

Blender and Onshape mouse presets are in **File → Settings**.

### Sketching

Select a plane (or not; you'll be asked) and press `S` for a new sketch. Double-click a sketch
in the feature tree to edit it again, and press `Ctrl+Enter` to finish.

| Action | Key |
|---|---|
| Line, rectangle, circle, arc | `L`, `R`, `C`, `A` (slot, polygon, point and centre rectangle are in the toolbar) |
| Smart dimension | `D`, then click one or two entities and click to place the value |
| Trim, offset, mirror | `T`, `O`, `M` (mirror uses the last selected line as the axis) |
| Horizontal, vertical, equal | `H`, `V`, `E` with geometry selected; all relations are in the properties panel |
| Construction geometry | `X` (toggles the selection, or the mode for new geometry) |
| Cancel / back to select | `Esc` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y` |

While drawing, points snap to existing points, midpoints, centres and curves, and lines snap
horizontal, vertical, tangent or perpendicular; the matching relations are added automatically.
Hold `Ctrl` to place a point without them. Geometry is blue while it can still move and
black/white once fully defined; conflicting or redundant relations turn red, and the
properties panel says which ones clash. Dimensions accept expressions such as `width / 2 + 5`,
using the named values from **Tools → Parameters** and other dimensions' names (`d1`, `d2`, …).

## Layout

```
crates/
  peet-math/      f64 geometry primitives and the tolerance model
  peet-platform/  time, storage locations and crash reports for native and web
  peet-render/    wgpu viewport renderer: camera, grid, meshes, edges, overlays
  peet-sketch/    2D sketches: entities, constraints, solver, expressions, editing operations
  peet-ui/        egui application shell: panels, commands, viewport interaction
  peetcad/        the application binary (native main and web entry point)
```

Core crates have no UI dependency and are tested headless: `cargo test --workspace`
(shader validation included, no GPU needed).

## License

[MIT](LICENSE)
