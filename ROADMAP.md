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
| File watching | `notify` (desktop only) | Tells an open assembly when a linked part's file changes |
| Profiling | Built-in performance overlay, `tracy` for deep dives | Built in from day one |

### Workspace layout (initial)

```
crates/
  peet-math/        f64 vectors, transforms, tolerances, robust predicates
  peet-solve/       the constraint solvers' numerical core (added in Phase 7)
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

- [x] Feature tree (history) with an explicit dependency graph
- [x] Incremental regeneration: rebuild only features downstream of a change
- [x] **Persistent / topological naming**: stable references to faces and edges that survive upstream edits. This is the classic weak spot in FreeCAD. We will design it in from the start, not patch it later.
- [x] Rollback bar, suppress/unsuppress, reorder features, edit a feature in place
- [x] Clear, recoverable rebuild errors: a failing feature is flagged, and the rest of the model still displays
- [x] Global parameter/variable table and a units system (mm, inch, with unit-aware expressions)
- [x] Undo/redo for everything (command-based, with snapshotting where cheap)
- [x] Native file format (`.peet`): compact, versioned binary container with the parametric model plus optional B-rep and mesh caches for instant opening (see [Native file format](#native-file-format-peet))
- [x] Autosave and crash recovery (to disk natively, to IndexedDB on the web)
- [x] Reference geometry: planes, axes, points, coordinate systems

**Exit criteria:** Change a dimension in the first sketch, and a 20-feature part rebuilds correctly in under 100 ms.

**Status:** Implemented in `peet-model` (feature tree, dependency graph, naming, engine, undo history), `peet-kernel` (face provenance from booleans and extrusions), `peet-io` (the `.peet` container and document sections), `peet-platform` (open/save, autosave storage) and `peet-ui`. `crates/peet-model/tests/exit_criterion.rs` builds a 20-feature bracket (sketches on faces of earlier features, cuts, a rib on an offset plane, a boss underneath, a reference plane), changes the width in Sketch1 five times and checks every feature builds and the volume matches the hand-calculated value each time. That rebuild takes about 5 ms in a release build (`cargo bench -p peet-model`), against the 100 ms budget. The design is recorded in [ADR 0001](docs/adr/0001-persistent-naming.md) (naming by feature origin), [ADR 0002](docs/adr/0002-incremental-regeneration.md) (content-keyed rebuilds and snapshot undo) and [ADR 0003](docs/adr/0003-native-file-format.md) (the file format). Still to do: a hands-on pass in the running app. The tree's drag and drop, picking references in the viewport, the dialogs and every web-only path (file picker, IndexedDB autosave) have been compile-checked but not exercised by hand. Known limits: undo while a sketch is open undoes within the sketch; sketches on faces don't yet project the face's edges; an angled plane turns about an axis feature or a standard axis, not directly about an edge; the 100 ms budget is only enforced in release builds and the benchmark, not in CI.

### Phase 4: Sheet Metal MVP ⭐
*Goal: design a real sheet metal part and send its flat pattern to a laser cutter.*

- [x] Sheet metal part settings: thickness, default bend radius, K-factor, relief type
- [x] **Base flange / tab**: from a closed sketch (plate) or an open sketch (profile with automatic bends)
- [x] **Edge flange**: select an edge, set length, angle and offset (inner/outer/material-inside), and drag to set length
- [x] Bend reliefs: rectangular, obround, tear
- [x] **Cuts** through sheet metal, including across bends (cut normal to the sheet)
- [x] **Unfold / fold** toggle and live **flat pattern** view
- [x] Bend allowance models: K-factor, bend allowance and bend deduction, with values shown per bend
- [x] **DXF export** of the flat pattern: outer profile, cutouts, bend lines on a separate layer, bend annotations (direction, angle, radius)
- [x] Bend table / flat pattern report (bend order, angles, flat size)

**Exit criteria:** Model an enclosure panel with 4 flanges, reliefs and cutouts, export the DXF, and it is dimensionally correct against hand-calculated values.

**Status:** Implemented in the new `peet-sheetmetal` crate, with features in `peet-model` (`sheet.rs`), DXF in `peet-io` and the UI in `peet-ui`. Sheet metal is native and flat first: a body carries a layout of flanges and bend strips in flat-pattern coordinates. One build produces the flat solid and a folded solid with *the same topology*, so unfolding is exact and free, and references and picking work in either view ([ADR 0004](docs/adr/0004-native-sheet-metal.md)). `crates/peet-io/tests/sheet_metal_exit.rs` models the enclosure panel (200 × 150 × 1.5 mm, R2, K 0.44; four 25 mm flanges set back 10 mm with rectangular reliefs; a window, four holes, a hole in a flange and a slot across a bend). It exports the DXF, reads it back and checks it against the hand calculation: blank 244.356636 × 194.356636 mm, every hole centre including the unfolded flange hole, bend-line positions and lengths, the slot's 20 mm flat length and the bend notes. 24 tests in `peet-sheetmetal` (including a property test over random flanges that checks validity and volume at K = 0.5) and 10 in `peet-model/tests/sheet_metal.rs` cover positions, up and down bends, reliefs, flanges on flanges, open profiles, cuts across bends, overlaps and references that survive edits. The panel rebuilds in about 4 ms in a release build. It was also checked by hand in the running app (**File → Open Sample Enclosure Panel**): folding, the flat pattern view with bend lines, the bend table, the panels and dragging a flange's length. That check found and fixed an older bug: bodies were not drawn after a part was opened or created, because a new document could repeat the old one's revision number. Known limits: corners are left open (bend zones that meet are reported as an overlap; closed corners are Phase 5); straight cut edges cross bends only parallel or square to the bend line, and round holes across bends are refused; open profiles take lines only (no arcs); an edge flange spans one edge (selecting several edges makes one flange each); Extrude or Cut-Extrude on a sheet body turns it into a plain solid, with a warning.

### Phase 5: Sheet Metal complete + Interop
*Goal: sheet metal on par with mainstream tools for everyday work.*

- [x] Miter flange (flanges along a chain of edges)
- [x] Hem (closed, open, teardrop, rolled)
- [x] Jog, sketched bend (bend along a sketch line)
- [x] Closed and open corners, corner reliefs, corner gap control
- [x] Flange on a partial edge, flange profiles from a sketch
- [x] Gauge / material tables (thickness, radius and K-factor by material, user editable, shareable)
- [x] Mirror and pattern (linear, circular) of features
- [x] Simple forming features: louvers, embosses, dimples (represented in the flat pattern)
- [x] **STEP export (AP214/AP242)** of the folded solid
- [x] DXF import (for sketches and base flanges from existing profiles)
- [x] Manufacturing checks: minimum flange length, hole too close to a bend, overlapping flat pattern

**Exit criteria:** A user can reproduce a typical industrial sheet metal bracket or chassis, and a fabricator accepts the DXF and STEP without rework.

**Status:** Implemented on the flat-first layout of Phase 4, which carried all of it ([ADR 0005](docs/adr/0005-sheet-metal-phase-5.md)). In `peet-sheetmetal`: a flange is a *profile* (a chain of bends and flats), which gives edge flanges, every kind of hem and mitre flanges with a sketched profile; sketched bends and jogs split a flange in place without moving anything in the flat pattern; corners between flanges are recorded and resolved at build time (butt, overlap or open, a gap, a rectangular or tear relief), with lips in one plane mitred exactly; dimples, embosses and louvers are square-walled forms present in both the flat and the folded solid; manufacturing checks and gauge tables. In `peet-model`: the features, plus linear and circular patterns and mirrors of extrusions, cuts, sheet metal cuts and forms, each copy's faces named apart. In `peet-io`: STEP export as an exact B-rep, DXF import into sketches, forms on the flat pattern DXF. In `peet-ui`: commands, ribbon tools and property panels for all of it, a checks window and a materials window.

`crates/peet-io/tests/chassis_exit.rs` builds the exit part, a chassis tray (`samples::chassis`, **File → Open Sample Chassis**): a 240 × 160 × 1.5 base with a mitre-flanged rim on three sides (40 mm walls, 12 mm lips, mitred), a closed hem on the front edge, four holes and ten louvers from patterns, two dimples from a mirror and a window in the back wall. Its DXF is checked against the hand calculation (blank 333.571680 × 215.876415 mm; hole centres; the window's position through two bends; every bend line; the lances; the notes), it passes the manufacturing checks, it reads back as one closed blank with five holes, and it survives save and reopen. Its STEP file (AP214 and AP242, folded and flat) is checked face by face in `step_export.rs`: every edge used twice, every loop the right way round, areas and volume equal to the kernel's. The chassis rebuilds in about 4 ms. The workspace has 61 tests in `peet-sheetmetal`, 9 in `peet-model/tests/phase5.rs`, 5 in the exit test and 7 for STEP, besides those of earlier phases; all pass, clippy is clean and the web build compiles.

Still to do: the STEP files have not been opened in another CAD system (none was available to the tests: they are checked by a Part 21 reader written for the purpose), and a fabricator has not seen the DXF, so the second half of the exit criterion is unconfirmed. The new commands, ribbon tools, panels and windows have been compile-checked and their features tested through the model, but none has been exercised by hand in the running app. Known limits: "flange on a partial edge" is the edge flange's offsets from Phase 4 (with reliefs), not a new feature; a mitre flange's edges must be on one face; corners are exact for square corners with square bends, and other angles may be refused; corner reliefs are rectangular or tear (a round relief would cross the bends); lips turned outwards round a corner overlap in the flat pattern and are refused; forms have square walls and sharp corners (rounded ones need the Phase 6 surfaces); patterns and mirrors copy extrusions, cuts, sheet metal cuts and forms, not flanges, and a sheet metal copy must stay on its original's face; a jog keeps the flat length, so the far side moves in; hems don't make corners with their neighbours (the sheet tears at their ends unless they are set back); DXF import skips splines and ellipses; the gauge tables' stainless and galvanised thicknesses should be checked against a supplier's chart.

### Phase 6: General solid modelling
*Goal: broaden beyond sheet metal.*

- [x] Surface types: cone, sphere, torus
- [x] Boolean operations (union, subtract, intersect) on the analytic surfaces: every pair turned about one axis, planes against spheres, cones and tori, and equal cylinders that cross. *Curved faces that cross without a common axis are refused with a message; they need the freeform kernel.*
- [x] Revolve, and sweep along a path of lines and arcs, with mitred corners between straight pieces
- [x] Hole wizard (standard metric sizes, counterbore/countersink, drill point, threads as cosmetic)
- [x] Fillet and chamfer (constant radius, edge chains): straight edges between flat faces and round edges between faces turned about the edge's axis
- [x] Shell, draft (flat faces, and round faces along the pull)
- [x] STEP import of analytic solids, with units, assemblies' placements and seams rebuilt
- [x] Measurement tools, mass properties (volume, mass, centre of gravity, moments of inertia)
- [x] **NURBS** curves and surfaces as kernel geometry, with booleans between freeform faces and any others by marched intersection curves (which also covers the curved analytic faces that cross without a common axis)
- [x] Loft through closed profiles on planes (two joined straight, more smoothly)
- [ ] Sweep along a general (spline or 3D) path: *needs splines in the sketcher*
- [ ] Fillets, chamfers, shells and drafts on freeform faces: *need offset surfaces and rolling-ball blends*
- [x] STEP import of freeform faces
- [x] Convert solid to sheet metal (walls, bends, holes and reliefs recognised in a solid of one thickness)

**Exit criteria:** A turned and drilled part (the bearing housing sample) is modelled with these features, matches its hand-calculated volume, and goes out to STEP and comes back as the same solid.

**Status:** The analytic half of the phase is implemented, and of the freeform half: NURBS geometry, freeform booleans, lofts, freeform STEP import, and converting solids to sheet metal. Sweeps along spline paths and fillets, shells and drafts on freeform faces are not started. The design is recorded in [ADR 0006](docs/adr/0006-general-solid-modelling.md) (analytic) and [ADR 0008](docs/adr/0008-freeform-geometry.md) (freeform).

In `peet-kernel`: cones, spheres and tori are surfaces of revolution with one parametrisation, so areas, volumes, tessellation and booleans treat them alike; faces stay polygons in parameter space, with seams for full turns and the poles (a sphere's, a cone's apex) always at vertices. `revolve` builds turned solids directly. `blend` makes fillets and chamfers by sweeping the sliver between an edge's faces and the blend, then subtracting or adding it, with mitres where blended edges meet and a ball where three fillets meet squarely. `reshape` offsets faces, shells and drafts by giving faces new surfaces and solving the vertices and edges again on the same topology. `query` gives mass properties and measurements. In `peet-model`: revolve, sweep, hole, fillet, chamfer, shell, draft and imported-body features, with persistent names for their faces, and patterns and mirrors of revolves and holes. In `peet-io`: STEP export of the new surfaces and STEP import. In `peet-ui` and `peet-document`: commands, ribbon tools, property panels, the mass properties window and measurements in the selection panel.

`crates/peet-io/tests/housing_exit.rs` builds the exit part (`samples::housing`, **File → Open Sample Housing**): a flange and hub revolved from one half section, a 4 mm fillet at the foot of the hub (a torus), 1.5 mm chamfers on the top rims (cones) and six M8 counterbored holes on a bolt circle from a circular pattern. Its volume is 124 840.83 mm³ by hand (Pappus's theorem) and in the model, to rounding; its STEP file (AP214 and AP242) reads back with the same faces, edges, volume and area; the imported body takes a further fillet; it survives save and reopen with its B-rep cache. It rebuilds from scratch in about 19 ms in a release build. The workspace gains 37 tests in `peet-kernel` (revolve and booleans on the new surfaces, blends, offset/shell/draft, mass properties, silhouettes), 14 in `peet-model/tests/phase6.rs`, 33 for STEP import and 4 in the exit test; everything from earlier phases still passes.

Still to do: the new commands, panels and windows have been compile-checked and their features tested through the model, but none has been exercised by hand in the running app. STEP files from other CAD systems have only been imitated in tests (hand-written files in their styles), not read from real exports. Known limits: every edge of one fillet or chamfer takes the same size; a straight edge on a curved face, an elliptical edge, and an edge that ends on a curved face cutting across it can't be blended; fillets of different radii can't meet at a corner; three fillets meet in a ball only at square convex corners (elsewhere they mitre); shell and draft are refused when the topology would change (a wall thicker than a neighbouring fillet, a drafted wall next to a tangent round face); draft tilts flat faces only; a sweep's path must be smooth, flat and made of lines and arcs, and the profile must be square to its start; a hole can't run into another curved face off its own axis (a cross-drilled hole, a counterbore that reaches a fillet); a plane can cut a cone only square to its axis, through its apex or in an ellipse, and a torus only square to its axis or through it; threads are a designation on the hole, not drawn; the standard hole sizes should be checked against the fasteners used; the centre of gravity and inertia of curved bodies come from a fine mesh (about one part in 10⁴); imported bodies are stored in the model as they came, with no healing of loose tolerances, and a large STEP file is read on the UI thread; a sweep's path sketch can only be pre-selected while the profile sketch is open (the tree has no multi-selection).

The freeform stage adds, in `peet-kernel`: `nurbs` (clamped rational B-spline curves and surfaces, interpolation and skinning), `loft`, and `boolean/freeform` (marching intersections fitted to a fifth of the modelling tolerance); every consumer of a surface (lifting, tessellation, silhouettes, validation, area and volume, ray casts, STEP export) handles the new kind. In `peet-model`: the loft feature and convert-to-sheet-metal; in `peet-sheetmetal`: `recognize`, which works a flat layout out of a solid and proves it by rebuilding the solid from the layout and comparing; in `peet-io`: B-spline curves and surfaces in STEP import. Loft, Cut-Loft and Convert have commands, panels and operations (`loft`, `cut_loft`, `convert_to_sheet`).

Known limits of the freeform stage: a boolean that involves a freeform face takes a few tenths of a second (analytic ones take microseconds); faces that touch along a curve without crossing can't be intersected; fillets, chamfers, shells and offsets refuse freeform faces and edges; draft doesn't tilt freeform faces; the sketcher has no splines, so freeform shapes come from lofts, intersections and import only; STEP import refuses freeform faces that come to a point where their surface is pinched together (a dome's tip written as a B-spline), bands that wrap a closed surface with no seam, offset surfaces and edges given only as parameter-space curves, and has only been tested on hand-written files in other systems' styles; a loft's profiles need the same number of edges, and there are no guide curves or end tangency; a sweep mitres corners between straight pieces only, and its path is still flat; converting to sheet metal refuses pressed forms, conical bends, sharp bends, and walls of different thickness; none of the new commands has been exercised by hand in the running app.

### Phase 7: Assemblies
*Goal: put parts together, hold them with mates, and report on the whole.*

The design is recorded in [ADR 0009](docs/adr/0009-assemblies.md). The work is in six stages:

**Stage 1: groundwork**
- [x] A part has a material, a density and a colour (they were application settings or constants)
- [x] `peet-solve`: the constraint solver's numerical core, out of `peet-sketch`, behind a trait for a system of equations
- [x] A session of several open documents, one current; operations apply to the current one, or to one they name

**Stage 2: assembly documents and instancing**
- [x] The assembly: definitions (parts embedded in the file, sub-assemblies), components with placements, saved in the model section of `.peet`
- [x] Assembly tree; drawing components with shared meshes; picking by instance
- [x] Insert, move, fix, replace, delete and open a component for editing (the part alone, not in context). *Moving is by typed position; dragging in the view comes with mates*
- [x] Operations, the `assemblies` skill and the reference
- [x] STEP import that keeps an assembly's structure; STEP export of an assembly
- [x] Linked parts: a component that follows a part file, by a path relative to the assembly (desktop only); the application watches the files and updates

**Stage 3: mates**
- [x] Coincident, concentric, distance, angle, parallel, and fasten (a rigid group), on persistent references
- [x] Solving with `peet-solve`: six unknowns per free component; rigid sub-assemblies
- [x] Conflicting mates told apart and explained; the freedom left in the assembly. *Redundant mates that agree are accepted silently*
- [x] Dragging a component in the view with a live solve (and the `drag` operation)
- [x] Freedom per component, to show which are still loose

**Stage 4: interference, bill of materials, mass**
- [x] Interference detection (bounding boxes, then the kernel's intersection), with volumes
- [x] Bill of materials: quantities, material, mass, sheet metal flat sizes; CSV export
- [x] Mass properties of an assembly

**Stage 5: visibility, colours, exploded views**
- [x] Show, hide and colour components
- [x] Exploded views as stored steps that don't affect mates

**Stage 6: large assemblies**
- [x] Instanced rendering, culling, loading from the mesh cache on demand, level of detail

**Exit criteria:** An enclosure is assembled from the chassis sample, a cover, the housing and patterned fasteners, fully mated, with no interference and a bill of materials that matches the hand count and hand-calculated masses and flat sizes. Changing the chassis's width moves every component to the right place. A mate solve during a drag takes under 4 ms with 50 components, and 1,000 instances of 50 parts draw at 60 FPS.

**Left out on purpose:** editing a part in the context of its assembly (features that refer to neighbouring parts), and flexible sub-assemblies.

**Status:** All six stages are implemented. The exit assembly (the chassis, a cover, the housing and patterned fasteners, fully mated) has not been built: the criteria are met piece by piece in tests, not yet on that one assembly, and the application has not been exercised by hand. Large assemblies: the instances of a part are drawn in one call each for faces and edges, what is outside the view is left out, a body is tessellated and uploaded when it is first seen, an assembly's file keeps the meshes that were drawn, and bodies small on screen get a coarser mesh and no edges. `crates/peet-render/tests/gpu.rs` draws 1,000 instances of 50 parts (1.9 million triangles) on the machine's graphics adapter in about 1 ms a frame (an RTX 4070 Super; the criterion is 16.7 ms) with 102 draw calls, and picks two instances of one mesh each in its own place; before this stage instances that shared a mesh were all drawn where the last one was. Components can be hidden, isolated and given a colour of their own, and an assembly has an exploded view: stored steps that each move some components by a distance, shown as a view that moves only where bodies are drawn (mates, interference, mass and exports use the assembly as it is put together; `crates/peet-ops/tests/assembly.rs` checks that showing it exploded changes no reply and not the model). In the application the view opens and closes over 0.4 s; this was tested without a window, not by hand. STEP carries an assembly's structure both ways: export writes each part and sub-assembly once as a product and each component as a placed occurrence of it, and import into an assembly makes parts, sub-assemblies and fixed components of what the file holds (`crates/peet-ops/tests/assembly.rs` sends a three-level assembly out and back and compares every component's place, the parts' counts and the volume); these files have only been read by our own importer, not by another CAD system. Interference between components is found by intersecting every two bodies whose boxes overlap, with the volume and place of each overlap; parts that only touch (a pin in a hole of its size, faces against each other) are not reported. The bill of materials counts each part through sub-assemblies, with material, mass, and thickness and flat size for sheet metal, and goes out as CSV. An assembly is weighed with each part's own material. `crates/peet-document/src/assembly.rs` checks these against hand values (a slug of π·4²·5 mm³ where a pin goes through a plate with no hole, the enclosure panel's 244.357 × 194.357 blank in the bill, a mass-weighted centre of gravity of three components). A component can be linked to its part's file instead of the assembly keeping a copy (desktop only): the link is relative to the assembly's folder by default; the file is read when the assembly is opened, on request, when the part is saved in the same session, and (in the application) when it changes on disk; the assembly keeps the part as last read, so it opens whole when the file is missing. `crates/peet-ops/tests/assembly.rs` covers a part changed behind the assembly's back, a missing file, a folder moved as a whole, and a linked part edited in its own file. Mates (coincident, concentric, parallel, distance, angle, fasten) join faces, edges and corners of two components' parts by persistent references, and are solved whenever the assembly is rebuilt: the components move as little as they can from where they are, groups of components that mates join are solved on their own, and a solved assembly is left bit for bit as it is. `crates/peet-model/tests/mates.rs` checks positions against hand values (plates stacked and squared up, a pin in a hole, an angle, a fastened pair carried along, a part turned over by `flip`), conflicts (the earlier mate holds, the later is flagged), mates that follow a face through an edit to the part, a mate to a part inside a sub-assembly, and 50 components held by 147 mates dragged at about 3 ms per step in a release build (budget 4 ms). `crates/peet-ops/tests/assembly.rs` covers the operations (`mate`, `edit_mate`, `mates`), and the command line's tests run the skill's mate script and check what the skill says about it. A component is dragged with the left mouse button, or by the `drag` operation: a point of it is pulled towards a place and it goes as far as its mates let it, sliding if it can and turning if it must, taking what it is mated to along (a plate on a pin swings a quarter turn; a stack of 50 plates is pulled by a corner at about 3 ms a step). In the application a click in an assembly picks a face, edge or corner, six mate commands join two picked ends, and the tree and properties show mates; tested without a window, not by hand.

Before mates: An assembly is a `Model` with an `Assembly` in place of features: definitions (whole part models, stored once) and components (instances with placements). The engine rebuilds each part with an engine of its own, only when that part changed, and gives instances that share their bodies; `crates/peet-model/tests/assembly.rs` checks that moving a component rebuilds nothing and that an edit to a part reaches every instance. `crates/peet-ops/tests/assembly.rs` covers the operations (`insert`, `place`, `fix`, `replace`, `open_component`, `components`), files, sub-assemblies, export and the translation of the application's changes; the `assemblies` skill's scripts run in the command line's tests. A part of an assembly is edited as a working copy in a document of its own and stored back with `save`, as one undo step of the assembly. In the application: New Assembly, Insert Part, Edit Part, a component tree and properties, and a viewport that draws instances with shared meshes; these were tested without a window, not exercised by hand. Each component reports how many ways it can still move, and the tree marks the ones that can. Known limits: a script can't describe a face inside a sub-assembly; mass properties, measurements and face selection don't work in an assembly yet; an assembly's parts are rebuilt when it is opened (its file caches their display meshes, not their geometry).

Stage 1: A part's `Model` has a material (name and density) and a colour, saved with it (model schema 5; older files still open) and set with `set_material` and `set_color`; the material tables carry densities, and `mass` gives the mass. The solver's core is the new `peet-solve` crate, which `peet-sketch` now uses: its 186 tests pass unchanged and the drag benchmark is the same to within noise. `peet_document::Session` holds the open documents; `peet_ops::apply_session` sends an operation to the current one or to one it names, with `documents`, `switch`, `close` and `"keep": true` on `new`, `open` and `open_sample`; the command line and the application hold a session. Still to do in this stage's area: the mass properties window's new material and colour rows and the document tabs have been compile-checked and tested through the operations, not exercised by hand; the application can't yet open a second document from its own menus (stage 2 adds the commands that do); autosave covers the current document only.

### Phase 8: Drawings
- [ ] 2D drawing sheets, templates, title blocks
- [ ] Projected, section and detail views, including the flat pattern view with bend notes
- [ ] Dimensions and annotations linked to the model
- [ ] PDF, DXF and SVG export

### Later / exploring
- Scripting and agent access: in progress, see [docs/scripting-plan.md](docs/scripting-plan.md). A scripting language (Rust plugins and/or embedded Python or Lua) comes on top of it
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
| Mate solve during a drag (50 components) | < 4 ms |
| Viewport frame time (1,000 instances of 50 parts) | < 16 ms |
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
| Native file format | Compact binary container (`postcard` + `lz4`), with regenerable geometry and mesh caches for fast opening ([ADR 0003](docs/adr/0003-native-file-format.md)) |
| Persistent naming | Faces are named by the feature and role that made them, carried through booleans; references add neighbours and a point to break ties ([ADR 0001](docs/adr/0001-persistent-naming.md)) |
| Sheet metal | Native and flat first: a layout of flanges and bends in flat coordinates; the folded and flat solids share their topology, so unfolding is exact ([ADR 0004](docs/adr/0004-native-sheet-metal.md)) |
| Regeneration and undo | Content-hashed keys per feature decide what to rebuild; undo stores model snapshots that share unchanged features ([ADR 0002](docs/adr/0002-incremental-regeneration.md)) |
| Sheet metal features | Flanges are profiles of bends and flats; sketched bends split a flange in place; corners are declared and resolved at build time; forms are square-walled and present in both solids; patterns copy features, not geometry ([ADR 0005](docs/adr/0005-sheet-metal-phase-5.md)) |
| General solid modelling | Analytic first: cones, spheres and tori as surfaces of revolution with poles at vertices; fillets and chamfers as boolean tools; shell and draft by solving the same topology on new surfaces; imported bodies as features. NURBS are one more kind of surface, with marched intersections ([ADR 0006](docs/adr/0006-general-solid-modelling.md), [ADR 0008](docs/adr/0008-freeform-geometry.md)) |
| Assemblies | A second kind of document in the same container; parts embedded in the assembly's file first, linked files second; components are instances of definitions; rigid sub-assemblies; no in-context editing; mates on persistent references, solved by the sketch solver's core ([ADR 0009](docs/adr/0009-assemblies.md)) |

## Open questions

1. Scripting language for users: Rust plugins, Python, Lua or Rhai? (Rhai and Lua are easy to embed in WASM. Python is much harder.)
2. Which sheet metal outputs matter most beyond DXF (for example specific press brake or laser software formats)?
3. Web hosting: a static site only, or eventually a backend for accounts and cloud storage?
