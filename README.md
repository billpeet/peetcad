# PeetCAD

A lightweight parametric 3D CAD application written in Rust, with sheet metal first.
It runs as a native Windows app and in the browser (WebAssembly), from one codebase.

> **Status:** early development (Phase 5: sheet metal for everyday work, with base, edge
> and mitre flanges, hems, sketched bends and jogs, corners, dimples, embosses and
> louvers, patterns and mirrors, manufacturing checks, gauge tables, a live flat pattern,
> DXF export and import and STEP export, on top of a parametric, history-based modeller
> with sketches, extrude and cut, reference geometry, undo and a native file format).
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

### Solids

With a sketch open or selected, click **Extrude** or **Cut** in the toolbar. The properties
panel sets the operation (new body, add, cut), the end condition (blind, mid-plane, through
all, up to a face) and the depth; the model rebuilds as you change them. A feature that fails
is flagged in the tree with the reason, and the rest of the model still builds.

Click a face or edge in the viewport to select it (hovering highlights it first). To sketch on
a model face, select a flat face and press `S`. **File → Export STL** saves the bodies as a
mesh.

### Sheet metal

Select a sketch and click **Base Flange** (toolbar or **Features → Sheet Metal**). A
closed shape makes a flat plate; a chain of connected lines makes a profile with a bend at
each corner. Its properties set the thickness, bend radius, bend model (K-factor, bend
allowance or bend deduction) and relief type.

| Action | How |
|---|---|
| Add a flange | Select one or more edges along the top or bottom face, then **Edge Flange** (or click it first and pick an edge). Set the length (measured on the outside), angle, position (material inside or outside, bend outside), offsets from the ends of the edge, and a custom radius. Drag the orange arrow on the selected flange to change its length |
| Cut through the sheet | Sketch on a flat face of the sheet, then **Cut**. The cut is made in the flat pattern, so straight edges can run across bends |
| Hem an edge | Select edges, then **Hem**: closed, open (with a gap), teardrop or rolled |
| Flange round several edges | Sketch a profile (connected lines) square to one of the edges, starting at its top or bottom corner: the face at the end of the edge is a handy place. Select the edges, then **Miter Flange**. Where the edges meet, walls butt and lips in one plane are mitred, with a gap |
| Bend along a line | Sketch lines across a flat face, then **Sketched Bend** (one bend per line) or **Jog** (a step: two bends that offset the far side) |
| Corners | Flanges on neighbouring edges meet in a butt corner with a relief and a 0.1 mm gap. **Corner** changes that for the selected flanges' faces, or for every corner: butt, overlap or open, the gap, and the relief |
| Dimples, embosses, louvers | Sketch circles (dimples) or closed polygons (embosses, louvers) on a flat face, then **Dimple**, **Emboss** or **Louver**. They are pressed, not cut: the flat pattern marks them on their own layer (a louver's open side is cut) |
| Patterns and mirrors | Select an extrusion, a cut, a sheet metal cut or a form in the tree, then **Linear Pattern** (a row or a grid), **Circular Pattern** or **Mirror**. Directions can be the standard axes, a reference axis or a picked edge |
| Check for manufacture | **Check**: flanges too short to bend, holes too close to a bend or an edge, holes too small, and parts that collide when folded. The limits are editable (multiples of thickness and radius) |
| Materials and gauges | **Materials**: tables of thickness, bend radius and K-factor by material and gauge. **Apply** a row to the part; edit the tables, or share them as CSV |
| Flat pattern | `U` toggles between folded and flat; bend lines are dashed |
| Bend table | **Bends**: flat size, and every bend's direction, angle, radius, K-factor, allowance and deduction (copy it as text for a spreadsheet) |
| DXF for the laser | **File → Export Flat Pattern DXF**: outline, cutouts, bend lines, bend notes and form marks on separate layers, in mm |
| STEP for other CAD | **File → Export STEP**: the folded part as exact geometry (AP214) |
| Start from a drawing | **File → Import DXF**: lines, arcs, circles and polylines come into the open sketch, or into a new one. A closed outline is ready for **Base Flange** |

**File → Open Sample Enclosure Panel** opens a panel with four flanges, reliefs and
cutouts. Try changing the `thickness` and `flange` parameters (**Tools → Parameters**).
**Open Sample Chassis** opens a tray with a mitre-flanged rim, a hem, patterned holes and
louvers and mirrored dimples.

### The feature tree

The tree is the part's history. Features refer to faces by what made them (not by number),
so a sketch on a face follows it when anything upstream changes, and only what is affected
is rebuilt (the status bar shows the last rebuild time).

| Action | How |
|---|---|
| Edit a feature | Select it; the properties panel edits it in place. Double-click a sketch to open it |
| Reorder | Drag a feature up or down (it can't move above what it uses) |
| Roll back | Drag the bar at the end of the tree up; new features are added there |
| Suppress, delete, roll back to here | Right-click a feature |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y`, for every change to the part |

A feature that can't be built turns red with the reason in its tooltip and properties; the
rest of the part still builds. **Features → Reference Geometry** adds planes (offset, at an
angle, mid plane), axes (along an edge, through a round face, where two planes meet),
points and coordinate systems; select a face, edge or vertex first to build on it, or use
the **Pick** buttons. **Tools → Parameters** holds named values usable in any value field
(`thickness * 2`) and the document units (mm, cm, m, inch, ft); values accept units too
(`1in + 5mm`).

### Files

**File → Save** writes a `.peet` file (a 20-feature part is about 1 KB, plus display data
that makes it open instantly, which you can turn off in Settings). **File → Open Sample
Bracket** opens a 20-feature example. Unsaved work is autosaved and offered back after a
crash. In the browser, files are opened with the browser's file picker, saved as
downloads, and autosaved to the browser's storage.

For diffs and debugging, a part can be turned into readable text and back:

```sh
peetcad dump part.peet part.ron
peetcad pack part.ron part.peet
```

## Layout

```
crates/
  peet-math/      f64 geometry primitives and the tolerance model
  peet-platform/  time, files, autosave storage and crash reports for native and web
  peet-render/    wgpu viewport renderer: camera, grid, meshes, edges, overlays
  peet-sketch/    2D sketches: entities, constraints, solver, expressions, editing operations
  peet-kernel/    B-rep kernel: planes and cylinders, extrude, booleans, validation, tessellation
  peet-sheetmetal/ sheet metal: flat layouts of flanges and bends, folding, flat patterns, bend math
  peet-model/     the parametric core: feature tree, persistent naming, rebuilds, sheet metal features, undo
  peet-io/        file formats: native .peet, STL export, DXF flat patterns
  peet-ui/        egui application shell: panels, commands, viewport interaction
  peetcad/        the application binary (native main and web entry point)
```

Core crates have no UI dependency and are tested headless: `cargo test --workspace`
(shader validation included, no GPU needed).

## License

[MIT](LICENSE)
