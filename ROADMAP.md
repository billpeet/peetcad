# PeetCAD Roadmap

A parametric 3D CAD application written in Rust, in the spirit of SolidWorks and FreeCAD.

**Guiding principles**

1. **Performant.** Interactive rebuilds, 60+ FPS viewport, instant startup. We measure performance and treat regressions as bugs.
2. **Lightweight.** A single native binary, few dependencies, no heavy C++ toolchain. Target download size under 30 MB.
3. **Easy to use.** Sensible defaults, few modal dialogs, direct manipulation, discoverable commands, and clear error messages when a rebuild fails.
4. **Sheet metal first.** Sheet metal is the first vertical we get right end to end, from sketch to flat pattern DXF sent to a laser cutter.

---

## Strategic Decisions

### The geometry kernel

The kernel is the hardest and riskiest part of any CAD system. There are three options:

| Option | Pros | Cons |
|---|---|---|
| OpenCascade via FFI | Mature, full featured | Large C++ dependency, hard to build, slow iteration. Goes against "lightweight" and "from scratch" |
| Existing Rust kernel (`truck`, `fornjot`) | Pure Rust, NURBS support | Immature, and we'd be bound to their API and roadmap |
| **Our own kernel, scoped to what we need** | Full control, small, fast | A large amount of work, with numerical robustness to earn |

**Recommendation: build our own B-rep kernel, but restrict its scope at first.** Sheet metal parts are almost entirely **planes and cylinders** (flat faces and bend regions), with lines, arcs and circles as edges. A kernel limited to *analytic* surfaces (plane, cylinder, then cone and torus) is far more tractable than a general NURBS kernel, and it covers sheet metal completely. General freeform surfaces come later.

Put the kernel behind a clean crate boundary so a different backend could be swapped in if needed.

### Sheet metal as a first-class model, not a post-process

Most CAD tools model a solid and then try to reverse engineer bends from it. PeetCAD will represent sheet metal parts **natively** as a *sheet definition*: a graph of flat faces connected by bends, plus thickness, K-factor and bend radius. From this one representation we derive both:

- the **folded 3D solid**, for display, assembly and export, and
- the **flat pattern**, for DXF and manufacturing.

This makes unfolding exact and trivial instead of a fragile geometric operation, and it is our main technical advantage.

### Technology stack (proposed)

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust (stable), Cargo workspace | |
| Math | `glam` (f32 for rendering), own `f64` types for geometry | Geometry is always `f64` |
| App framework | `eframe` (`winit` + `wgpu` + `egui`) | One code path for Windows desktop and the browser |
| Rendering | `wgpu` | DX12/Vulkan on Windows, WebGPU in the browser with a WebGL2 fallback |
| UI | `egui` | Immediate mode, small, fast to iterate, runs natively and in WASM. Revisit if it limits us |
| Serialization | `serde` + `postcard` | Compact binary native format (see below) |
| Compression | `lz4_flex` | Pure Rust, so it builds for WASM without a C toolchain, and it decompresses very fast |
| Web build | `trunk` / `wasm-bindgen` | |
| Testing | `cargo test`, `proptest`, snapshot tests (`insta`) | Heavy property testing on the kernel and solver |
| Profiling | Built-in performance overlay, `tracy` for deep dives | Built in from day one |

### Workspace layout (initial)

```
crates/
  peet-math/        f64 vectors, transforms, tolerances, robust predicates
  peet-sketch/      2D sketch entities + geometric constraint solver
  peet-kernel/      B-rep topology & geometry, extrude/cut, tessellation
  peet-sheetmetal/  sheet definition, bend math, fold/unfold, flat pattern
  peet-model/       parametric feature tree, dependency graph, regeneration
  peet-io/          native file format, DXF, STEP, STL, 3MF
  peet-render/      wgpu renderer, picking, grid, highlights
  peet-ui/          egui panels, tools, commands, interaction
  peetcad/          the application binary
```

Dependencies only point downward (`ui` → `model` → `kernel` → `math`). The core crates have no UI or GPU dependency, so they can be tested headless and reused (for a CLI or scripting, for example).

### Target platforms

| Tier | Platform | Meaning |
|---|---|---|
| 1 | **Windows desktop** (x86_64) | Built, tested and released on every commit |
| 1 | **Web browser** (`wasm32-unknown-unknown`) | Built and smoke tested on every commit, deployed as a static site |
| 2 | Linux, macOS | Expected to build (the stack is cross platform), but not actively tested or released |

Supporting the browser as Tier 1 from day one shapes the architecture:

- **No direct OS calls in core crates.** File access, clipboard, threads and time go through small platform traits, implemented natively and for the web (browser file picker / File System Access API, downloads for export, IndexedDB for autosave and recent files).
- **Threading is optional.** Core algorithms must run correctly on a single thread. Parallelism (`rayon`) sits behind a feature flag and is used natively. On the web it's enabled only where the page can use WASM threads (this needs cross-origin isolation headers), and it's off otherwise.
- **Long operations never block the UI.** Large rebuilds and file loads are chunked or run in a worker, because a blocked browser tab can't be cancelled.
- **Download size is tracked.** Target for the compressed `.wasm`: under 10 MB. Dependencies get checked for WASM compatibility (in particular, no C libraries).
- **Rendering stays within WebGPU/WebGL2 limits.** No native-only GPU features are required, but they can be used when available.

### Native file format (`.peet`)

The priorities are **compact** and **fast to open**. The design is a small binary container:

```
[magic "PEET" | format version | section table]
[section: document metadata      ]  units, material, author, thumbnail (PNG)
[section: parametric model       ]  parameters, sketches, feature tree  ← the source of truth
[section: B-rep cache (optional) ]  last rebuilt geometry
[section: mesh cache (optional)  ]  tessellated display meshes
```

- Each section is `postcard`-serialized and `lz4`-compressed independently, with its own schema version.
- **Opening a file is instant.** The mesh cache is shown right away, and the model rebuilds in the background (or the B-rep cache is used directly if the file's version and hash match). The caches can always be thrown away and regenerated, so they're omitted when saving with "minimum size".
- Unknown sections are skipped, so newer files degrade gracefully in older versions.
- Reading a file is fuzz tested. Corrupt or malicious files must produce an error, never a crash.
- For debugging and diffing, a CLI command dumps a `.peet` to readable RON/JSON and converts it back.

---

## Phases

Each phase ends with something usable and demonstrable. Estimates are rough and assume a small team.

### Phase 0: Foundations
*Goal: an empty but polished application shell.*

- [x] Cargo workspace, MIT `LICENSE`, CI: fmt, clippy, tests on Windows, plus a `wasm32` build
- [x] Web build via `trunk`, deployed automatically as a static site (GitHub Pages) on every push to the default branch
- [x] Platform abstraction layer: time, data locations, crash reports. Settings persist to a file natively and to localStorage on the web (through eframe). *File dialogs and background tasks move to Phase 3, when there are files to open.*
- [x] `peet-math`: points, vectors, planes, frames, rays, bounding boxes, tolerance model (`tolerance::LINEAR`, `tolerance::ANGULAR`)
- [x] `wgpu` renderer: gradient background, adaptive anti-aliased grid, shaded meshes with edges, translucent reference planes, MSAA, reversed-Z depth; orbit/pan/zoom-to-cursor camera (turntable and trackball)
- [x] Standard views with smooth transitions, and a view cube with 26 clickable views
- [x] `egui` shell: menu bar, toolbar, feature tree panel, property panel, status bar with cursor coordinates
- [x] Command system: every action is a named command with a shortcut, shared by menus, toolbar and a fuzzy command palette (`Ctrl+K`). *Undo hooks arrive with the document model in Phase 3.*
- [x] Logging, crash reporting, built-in performance overlay (`F3`). *We use our own overlay rather than `puffin`, because `puffin_egui` lags behind egui releases.*
- [x] Mouse presets (SolidWorks, Blender, Onshape) and a light/dark theme
- [ ] Startup: create the window before GPU initialisation finishes (see below)

**Exit criteria:** The app starts in under 300 ms on Windows and under 2 s in the browser, and shows a smooth, navigable empty 3D scene on both.

**Status:** Navigation, rendering and the shell meet the criteria (about 1 ms CPU per frame). Windows startup does not yet: a release build takes about 0.9 s warm to the first frame on a laptop with an RTX 4050. About 0.8 s of that is window creation and DX12 driver initialisation; our own code takes about 100 ms. Defaulting to DX12 only (instead of probing Vulkan, DX12 and GL) already saved about 150 ms. The remaining fix is to show the window immediately and initialise the GPU in parallel.

### Phase 1: 2D Sketcher
*Goal: a sketcher that is pleasant to use and correct.*

- [x] Entities: point, line, arc, circle, construction geometry, rectangle/slot/polygon helpers
- [x] Constraints: coincident, horizontal, vertical, parallel, perpendicular, tangent, equal, concentric, midpoint, symmetric, fix
- [x] Dimensions: distance, horizontal/vertical distance, radius/diameter, angle, all driving or driven
- [x] Constraint solver:
  - Graph decomposition into independent clusters, so only affected parts are solved
  - Newton–Raphson / Levenberg–Marquardt with a dogleg fallback
  - DOF analysis: color-code under, fully and over constrained geometry
  - Detect conflicting or redundant constraints and explain *which* ones
- [x] Interactive dragging with live solve (target: under 2 ms per solve for sketches with 100+ entities)
- [x] Automatic constraint inference while drawing (snap to horizontal, coincident, tangent…)
- [x] Trim, extend, offset, mirror, fillet (2D)
- [x] Profile/region detection: find closed loops and nested regions for features to use
- [x] Expressions and named parameters in dimensions (`width = 2 * height + 5`)

**Exit criteria:** A user can draw and fully constrain a realistic bracket profile without fighting the tool.

**Status:** Implemented in `peet-sketch` (headless, 165 tests including property tests) and sketch mode in `peet-ui` (19 tests that drive the tools with simulated input). The bracket test in `crates/peet-sketch/tests/bracket.rs` builds an L bracket with a fillet, two holes and a slot, reaches 0 DOF with no diagnostics, and rebuilds correctly after a dimension change. `solve_drag` takes about 0.01 ms on that bracket and 0.1–0.2 ms on 120–180-line sketches (`cargo bench -p peet-sketch --bench solver`). Still to do: a hands-on usability pass by a real user against the exit criterion. Known gaps: fillets only between two lines, no tangent-arc drawing tool, and sketch undo is per editing session (document undo comes in Phase 3).

### Phase 2: Kernel v0 (analytic B-rep)
*Goal: turn sketches into valid solids.*

- [x] Half-edge / winged B-rep topology: Solid → Shell → Face → Loop → Edge → Vertex
- [x] Geometry: plane, cylinder (surfaces); line, circle/arc (curves)
- [x] Topology validation (Euler checks, manifoldness, orientation)
- [x] Extrude (blind, symmetric, up-to) of sketch regions, including holes
- [x] Cut-extrude (planar/cylindrical booleans limited to what extrude and cut need)
- [x] Tessellation to render meshes with edges and silhouettes, cached per face
- [x] GPU picking of faces, edges and vertices, plus pre-selection highlighting
- [x] Sketch on face / on a reference plane
- [x] STL export (early, cheap win)

**Exit criteria:** Sketch → extrude → sketch on a face → cut, with correct and valid topology.

**Status:** Implemented in `peet-kernel` (B-rep, extrude, booleans, validation, tessellation; 96 tests including randomized boolean tests), `peet-model` (the extrude/cut feature), `peet-io` (STL) and picking in `peet-render`. `crates/peet-model/tests/exit_criterion.rs` runs the exit criterion headless twice (a plate with a pocket, through hole and boss; an L bracket with features cut from three directions) and checks validity, topology counts and exact volumes at every step. A 20-face body minus a box takes about 0.6 ms including validation. Still to do: a hands-on pass in the running app (extrude, cut, picking and STL export have only been tested headless or by the agents' GPU test). Known limits: crossing non-parallel cylinders (a hole drilled through another hole at an angle) are rejected with a clear error; "up to" only accepts planar faces parallel to the sketch; tessellation is cached per body, not per face; sketches on faces don't reference the face's edges yet; region selection is automatic (outer regions with their holes). Features are rebuilt from scratch on every change until Phase 3 adds incremental regeneration.

### Phase 3: Parametric core
*Goal: a proper parametric history-based modeller.*

- [ ] Feature tree (history) with an explicit dependency graph
- [ ] Incremental regeneration: rebuild only features downstream of a change
- [ ] **Persistent / topological naming**: stable references to faces and edges that survive upstream edits. This is the classic weak spot in FreeCAD. We will design it in from the start, not patch it later.
- [ ] Rollback bar, suppress/unsuppress, reorder features, edit a feature in place
- [ ] Clear, recoverable rebuild errors: a failing feature is flagged, and the rest of the model still displays
- [ ] Global parameter/variable table and a units system (mm, inch, with unit-aware expressions)
- [ ] Undo/redo for everything (command-based, with snapshotting where cheap)
- [ ] Native file format (`.peet`): compact, versioned binary container with the parametric model plus optional B-rep and mesh caches for instant opening (see [Native file format](#native-file-format-peet))
- [ ] Autosave and crash recovery (to disk natively, to IndexedDB on the web)
- [ ] Reference geometry: planes, axes, points, coordinate systems

**Exit criteria:** Change a dimension in the first sketch, and a 20-feature part rebuilds correctly in under 100 ms.

### Phase 4: Sheet Metal MVP ⭐
*Goal: design a real sheet metal part and send its flat pattern to a laser cutter.*

- [ ] Sheet metal part settings: thickness, default bend radius, K-factor, relief type
- [ ] **Base flange / tab**: from a closed sketch (plate) or an open sketch (profile with automatic bends)
- [ ] **Edge flange**: select an edge, set length, angle and offset (inner/outer/material-inside), and drag to set length
- [ ] Bend reliefs: rectangular, obround, tear
- [ ] **Cuts** through sheet metal, including across bends (cut normal to the sheet)
- [ ] **Unfold / fold** toggle and live **flat pattern** view
- [ ] Bend allowance models: K-factor, bend allowance and bend deduction, with values shown per bend
- [ ] **DXF export** of the flat pattern: outer profile, cutouts, bend lines on a separate layer, bend annotations (direction, angle, radius)
- [ ] Bend table / flat pattern report (bend order, angles, flat size)

**Exit criteria:** Model an enclosure panel with 4 flanges, reliefs and cutouts, export the DXF, and it is dimensionally correct against hand-calculated values.

### Phase 5: Sheet Metal complete + Interop
*Goal: sheet metal on par with mainstream tools for everyday work.*

- [ ] Miter flange (flanges along a chain of edges)
- [ ] Hem (closed, open, teardrop, rolled)
- [ ] Jog, sketched bend (bend along a sketch line)
- [ ] Closed and open corners, corner reliefs, corner gap control
- [ ] Flange on a partial edge, flange profiles from a sketch
- [ ] Gauge / material tables (thickness, radius and K-factor by material, user editable, shareable)
- [ ] Mirror and pattern (linear, circular) of features
- [ ] Simple forming features: louvers, embosses, dimples (represented in the flat pattern)
- [ ] **STEP export (AP214/AP242)** of the folded solid
- [ ] DXF import (for sketches and base flanges from existing profiles)
- [ ] Manufacturing checks: minimum flange length, hole too close to a bend, overlapping flat pattern

**Exit criteria:** A user can reproduce a typical industrial sheet metal bracket or chassis, and a fabricator accepts the DXF and STEP without rework.

### Phase 6: General solid modelling
*Goal: broaden beyond sheet metal.*

- [ ] Full robust boolean operations (union, subtract, intersect) on analytic surfaces
- [ ] Revolve, sweep (analytic cases first), hole wizard (standard sizes, counterbore/countersink, threads as cosmetic)
- [ ] Fillet and chamfer (constant radius, edge chains). A known hard problem, so scope carefully
- [ ] Shell, draft
- [ ] Surface types: cone, sphere, torus, then **NURBS** curves and surfaces
- [ ] Loft; convert solid to sheet metal (recognize bends from a shelled solid)
- [ ] STEP import (needs NURBS for general files)
- [ ] Measurement tools, mass properties (volume, mass, center of gravity)

### Phase 7: Assemblies
- [ ] Multi-part documents, part instancing, external references
- [ ] Mates/joints: coincident, concentric, distance, angle, and rigid groups (reusing the constraint solver in 3D)
- [ ] Assembly tree, component visibility and colors, exploded views
- [ ] Interference detection
- [ ] Bill of materials (with sheet metal flat sizes and material)
- [ ] Large assembly performance: instanced rendering, lazy loading, LOD

### Phase 8: Drawings
- [ ] 2D drawing sheets, templates, title blocks
- [ ] Projected, section and detail views, including the flat pattern view with bend notes
- [ ] Dimensions and annotations linked to the model
- [ ] PDF, DXF and SVG export

### Later / exploring
- Scripting API (Rust plugins and/or embedded Python or Lua) and a headless CLI for batch export
- Installable Progressive Web App (offline use in the browser)
- Share-by-link: open a read-only model from a URL in the web build
- Configurations / design tables (part families)
- Nesting of flat patterns on sheet stock
- CAM for laser/plasma toolpaths, press brake bend sequencing
- Collaboration and version control friendly workflows (using the RON/JSON dump for diffs)

---

## Cross-cutting concerns

### Performance budgets
| Operation | Target |
|---|---|
| Cold startup to usable window (Windows) | < 300 ms |
| Page load to usable app (web, warm cache) | < 2 s |
| Compressed `.wasm` download | < 10 MB |
| Open a typical `.peet` file (to first display) | < 50 ms |
| Viewport frame time (typical part) | < 8 ms (120 FPS capable) |
| Sketch solve during drag | < 2 ms |
| Regenerate a 20-feature part after an edit | < 100 ms |
| Unfold / flat pattern | < 20 ms |
| Memory for an empty document | < 100 MB |

Benchmarks (`criterion`) run in CI for the solver, kernel operations and regeneration, so regressions are visible.

### Robustness and correctness
- Every kernel operation must produce valid topology or return a clean error. It must never panic or produce corrupt geometry.
- Property based and fuzz tests for the solver, booleans and unfolding.
- A **golden model corpus**: reference parts that must rebuild identically on every commit.
- Numerical strategy: consistent tolerances, robust geometric predicates, and no ad hoc epsilons scattered through the code.

### Usability
- Selection first *and* command first workflows.
- In-viewport handles and drag previews for common parameters (flange length, extrude depth).
- A searchable command palette.
- Every error message says what failed, where, and how to fix it.
- Keyboard and mouse navigation presets (SolidWorks, FreeCAD, Blender style).

### Project hygiene
- Architecture Decision Records (`docs/adr/`) for major choices (kernel, naming scheme, file format).
- Semantic versioning of the file format from the first saved file.
- **MIT licensed.** New dependencies must be license compatible (MIT, Apache-2.0, BSD, zlib and similar). No copyleft dependencies are linked into the binary.

---

## Key risks

| Risk | Mitigation |
|---|---|
| Kernel robustness (booleans, fillets) consumes the project | Restrict to analytic geometry first. Sheet metal needs little general boolean work. Keep the kernel behind a clean interface |
| Topological naming breaks models on edit | Design persistent naming in Phase 3, and test it against the golden corpus |
| Constraint solver is unstable or slow on real sketches | Graph decomposition, good initial guesses, extensive test corpus, and borrow proven techniques from SolveSpace and PlaneGCS |
| STEP is a large standard | Export first (a much easier subset), and import only after NURBS exists |
| Scope creep toward "full SolidWorks" | Each phase has explicit exit criteria. Sheet metal quality comes before breadth |
| UI toolkit limits (egui for complex CAD UI) | Keep UI logic separate from the model so the toolkit can be replaced |
| Web platform limits (no threads without cross-origin isolation, memory caps, weaker GPUs) | Single-threaded correctness first, WebGL2 fallback, web build in CI from day one so problems show up early |
| File format lock-in from early mistakes | Per-section schema versions, migration tests against saved files from every release, caches that can always be regenerated |

---

## Decisions log

| Decision | Outcome |
|---|---|
| Target platforms | Windows desktop and web browser (WASM) are both Tier 1 from day one |
| License | MIT |
| Native file format | Compact binary container (`postcard` + `lz4`), with regenerable geometry and mesh caches for fast opening |

## Open questions

1. Scripting language for users: Rust plugins, Python, Lua or Rhai? (Rhai and Lua are easy to embed in WASM. Python is much harder.)
2. Which sheet metal outputs matter most beyond DXF (for example specific press brake or laser software formats)?
3. Web hosting: a static site only, or eventually a backend for accounts and cloud storage?
