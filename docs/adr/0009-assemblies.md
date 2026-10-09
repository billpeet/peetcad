# ADR 0009: Assemblies

**Status:** accepted (Phase 7). Stage 1 (groundwork), most of stage 2 (assembly
documents) and most of stage 3 (mates) are implemented; the rest is the plan.

## Context

Until Phase 7 a document is one part: `Document` wraps one `Model`, the application
holds one document, an operation is applied to it, and `peet --file` names one part. An
assembly places several parts (and other assemblies) relative to each other, holds them
together with mates, and reports on the whole: interference, mass, a bill of materials.

Both Tier 1 platforms have to work the same way. The browser has no file paths, so a
design that depends on a folder of part files next to the assembly doesn't carry over.

## Decision

**An assembly is a second kind of document.** A part's source of truth is its features;
an assembly's is its components, mates and explode steps. Both are a `Model`: an
assembly's has no features and an `Assembly` in their place (`Model::assembly`). So
everything built on a model holds for both without a second copy: the document, undo by
snapshots that share what didn't change, the content hash that says whether a file is
modified, the parameters and units, and the file's model section. A model that has both
features and an assembly is refused as damaged. Operations are for one kind or the
other, and say so when sent to the wrong one. The model section's schema version went up
with it (6), so an older version says the file is from a newer one instead of
misreading it.

**Parts are embedded first, linked second.** An assembly file holds the parts it uses:
each is a complete part (its model, and its caches) stored in the assembly's file. One
file opens anywhere, headless or in the browser. A component can instead be *linked* to
a part file, on the desktop: the assembly then stores the file's path and the part as
it last read it, so it still opens when the file is missing (see "Linked parts" below).

**A component is an instance of a definition.** The assembly lists its definitions
(parts, sub-assemblies) once; a component names a definition and has a placement (a
rigid `Frame`), a name, and whether it is fixed. Any number of components share one
definition, and so one rebuild, one tessellation and one GPU mesh. A definition is a
whole `Model`, held by the assembly; inserting a model the assembly already has (the
same content) uses the definition that is there. A definition no component uses is
dropped.

**The engine rebuilds an assembly's parts with engines of its own**, one per definition,
and only when a definition's model is another than last time: moving a component
rebuilds nothing. The result is a list of instances (a body, where it is, the path of
components down to it), not bodies of the assembly's own; the document shows one entry
per instance, the instances of one body sharing a view.

**Sub-assemblies are rigid.** A sub-assembly is solved on its own and placed as one
body in its parent. Flexible sub-assemblies are left for later.

**A part is edited on its own, not in context.** Opening a component for editing shows
the part alone. A part's features can't refer to its neighbours in the assembly.
In-context references are where most of the complexity of assemblies in other systems
comes from (circular updates, references that dangle when a component is replaced), and
nothing in the phase needs them.

**Mates refer to geometry by instance and persistent reference.** A mate's ends are a
component plus the `FaceRef` / `EdgeRef` / `VertexRef` a pick or a selector gives
([ADR 0001](0001-persistent-naming.md)), so mates follow the geometry through edits to
the part, and a mate whose reference is lost is flagged as a failed feature is. The
kinds are those of the roadmap: coincident, concentric, distance, angle, parallel, and
*fasten*, which makes components one rigid group.

**Mates are solved by the sketch solver's numerical core.** The core (damped least
squares with minimum-norm steps, the dogleg fallback, the sparse Cholesky, row dropping
for redundant equations) moves out of `peet-sketch` into `peet-solve`, behind a trait
for a system of equations over a flat array of values. The sketcher's equations are one
implementation, unchanged in behaviour; the assembly's are another, with six unknowns
per free component (a translation and a rotation vector about the current orientation)
and residuals in millimetres or dimensionless, as in the sketcher. Dragging a component
uses the same two-phase drag, so everything else moves as little as it can.

**A part says what it is made of and what colour it is.** The bill of materials and the
mass of an assembly need a material and a density per part, and components need
colours. These are properties of the part's `Model` (not of the application's settings,
where the density was until now), with the application's material tables supplying the
values when a material is chosen from them.

**A session holds several documents.** The application and the command line work on a
`Session` (`peet-document`): the open documents, one of them current. An operation is
applied to the current document unless it names another (`"document"`), so every
existing script still means what it meant. A session dereferences to its current
document, so code written for one document works on a session unchanged. Opening a
component for editing will make its part current.

**A part of an assembly is edited as a working copy.** `open_component` opens the
definition's model as a document of the session, marked as embedded in its assembly.
It has its own undo history while it is open. `save` on it stores its model back as the
definition's, which is one undo step of the assembly ("Edit Bracket") and reaches every
instance; until then the assembly is unchanged. This is how a sketch is edited too: a
working copy, written back when it is finished. Closing the assembly closes the parts
opened from it, and refuses while one has changes that were not stored.

**Operations, skills and the reference grow with it**, as `AGENTS.md` requires: every
assembly command is an operation, and a new `assemblies` skill covers them.

## Stages

1. **Groundwork.** Material, density and colour on the part; `peet-solve`; the session.
2. **Assembly document and instancing.** The assembly model, its file section, the
   tree, drawing with shared meshes, picking by instance, operations and the skill, STEP
   import that keeps the structure, STEP export of an assembly. Then linked parts.
3. **Mates.** The equations, the solve, dragging, degrees of freedom and conflicts.
4. **Interference, bill of materials, mass.** Bounding boxes first, then the kernel's
   intersection; quantities, material, mass and flat sizes; CSV.
5. **Visibility, colours, exploded views.** Explode steps are stored per assembly and
   don't affect mates.
6. **Large assemblies.** GPU instancing, culling, loading from the mesh cache on demand,
   coarser meshes at a distance.

## What stage 1 built

- `peet_model::Material` and `Model::material`, `Model::color` (model schema 8 once merged
  with configurations; schema 6 and 7 files are read as `ModelV7`). Densities in the material tables. Operations
  `set_material` and `set_color`; `mass` gives `mass_kg`. The mass properties window
  sets the part's material and colour.
- `peet-solve`: `Problem`, `Options`, `Space` and the sparse Cholesky, generic over an
  `Equation` trait. `peet-sketch` implements it for its equations; its tests pass
  unchanged and its drag benchmark is the same to within noise (bracket 0.009 ms, 180-line
  truss 0.29 ms, before and after).
- `peet_document::Session`, `peet_ops::apply_session`, the operations `documents`,
  `switch`, `close` and `keep` on `new`, `open`, `open_sample`. The command line and the
  application hold a session; the application shows a tab per document when more than
  one is open.

## What stage 2 built so far

- `peet_model::Assembly` (definitions, components), `Model::assembly`, in model schema 8
  with the material and colour, instances in the `Evaluation`.
- `Document::placed` (where each shown body is, and whose), `Document::embedded`.
- Operations: `new` with `assembly`, `insert` (from a file, a sample, an open document
  or another component), `place`, `fix`, `replace`, `rename` / `suppress` / `show` /
  `delete` with `component`, `open_component`, `components`; `status` and `failures`
  for assemblies; STEP and STL export of every component where it is. The application's
  own changes to an assembly are translated to these, like its changes to a part.
- The `assemblies` skill and the reference.
- In the application: New Assembly, Insert Part and Edit Part; a component tree and a
  component's properties (name, position, quarter turns, fixed); components drawn
  where they are, the instances of a part sharing a GPU mesh; a click picks a component.

STEP keeps the structure both ways. Export (`peet_io::step::write_assembly`) writes
each part and sub-assembly once, as a product with a shape of its own, and each
component as a `NEXT_ASSEMBLY_USAGE_OCCURRENCE` with a
`CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` whose transformation takes the part's origin
to the component's placement: the layout OCCT and the commercial systems write. Import
(`StepImport::nodes`) gives each product once with what is placed in it;
`Document::import_step_assembly` makes parts (an import feature each) and
sub-assemblies of them and places the top level's children as fixed components, as one
undo step. A part still takes the same file as bodies where they are. The file has no
mates, so imported components are held, not mated. Also open: mass properties
and measurements in an assembly (stage 4), an assembly's caches in its file (it is
rebuilt when opened), selections on a component's faces (stage 3 needs them for mates),
autosave of documents other than the current one, and a colour per component (a part's
colour is baked into its mesh, so it needs a colour per drawn object).

## Linked parts (stage 2, desktop only)

- A definition has an optional `Link`: the path of the part's file. It still holds
  the part's model, as it was when the file was last read: that copy is what the
  assembly is built from, so the engine, undo, mates and the file format treat linked
  and own parts alike, and an assembly whose linked file is missing still opens whole,
  with a warning.
- The file is read when the assembly is opened (before the document is made, so it is
  not an unsaved change and not an undo step), by `update_links` (an undo step), and
  when the part's document is saved in the same session (an undo step of each assembly
  that links to it, "Update Bracket"). Linked assemblies read their own linked parts
  the same way, to a fixed depth.
- **Links are relative by default.** While an assembly is open a link is the file's
  full path, so undo snapshots, an assembly that has no file yet and Save As all mean
  the same file without anything being rewritten. A relative link is made relative to
  the assembly's folder only as the model is written (`Model::for_file`), and full
  again as it is read (`Model::from_file`); so an assembly and its parts moved or
  copied together, keeping their layout, find each other, and an assembly saved
  somewhere else still points at the same parts. A link can be absolute instead (a
  shared library that stays put). Paths on another drive stay full paths. Failing all
  that, a file of the same name next to the assembly's file is used.
- **Watching.** In the application, the system reports changes in the folders the
  open assemblies' linked parts are in (the `notify` crate: `ReadDirectoryChangesW`,
  inotify, FSEvents), and a part whose file was touched and has then been left alone
  for 0.3 s is read again, as `update_links` does: an undo step, with a line in the
  status bar. Folders are watched, not files, so a save that writes a new file and
  renames it over the old one is seen, and so is a missing file being put back. If the
  system's watcher can't be started, the files' times and sizes are compared once a
  second instead. It can be turned off in the settings. `notify` is in the desktop
  build only (`peet-ui`, not for `wasm32`); it is CC0-1.0, a public domain dedication,
  which the licence policy's "and similar" is taken to cover.
- `open_component` on a linked part opens the file as an ordinary document of the
  session (not a working copy): its `save` writes the file.
- Operations: `insert` with `link`, `update_links`, `link` (to an existing file, or
  writing the part out), `unlink`; `components` reports each link's state. In the
  application: Insert Linked Part, Update Linked Parts, and the link with "Make Own
  Copy" in a component's properties.
- It is refused in the browser build, where there are no files to follow. The `Host`
  is not involved after all: operations already read files from disk directly, and a
  browser host has nothing to answer with.

Not done: `replace` always copies; a part file that is open with unsaved changes is
read from disk, not from the open document; a part put back into a folder that did not
exist when the assembly was opened is noticed only at Update Linked Parts or when the
assembly is opened again (there was no folder to watch).

## What stage 3 built so far

- `peet_model::Mate`: a kind (coincident, concentric, parallel, distance, angle,
  fasten), two ends, `flip`. An end is the path of components down to a part and a
  `FaceRef`, `EdgeRef` or `VertexRef` in it. A flat face stands for its plane, a round
  face or a round edge for its axis, a straight edge for a line, a vertex (or a sphere)
  for a point.
- The solve (`peet-model/src/mate.rs`, where the formulation is written up). Each
  component that is not fixed has six unknowns: its origin, and a rotation vector from
  the orientation it had when the solve started, scaled by the component's size so that
  the Jacobian's entries are of order one. Every mate is made of four kinds of residual
  (two directions at an angle; a point at a distance along a direction of the other
  end; a distance between points; a distance from a line), with analytic gradients,
  checked against finite differences. `peet-solve` solves them with minimum norm steps,
  so the components move as little as they can.
- **Where a component is, is the suggestion.** The mates are solved whenever the
  assembly is rebuilt, from where the components are, and the placements they come to
  are stored in the components (as a sketch's solved points are stored in the sketch).
  So placing a mated component is a request, and there is no separate "solved" state to
  keep in step.
- **Groups.** Components joined by mates are solved as a group of their own; a fixed
  component joins nothing. A group whose mates already hold is not touched, so a solved
  assembly is bit for bit the same after any number of rebuilds (which matters for
  "is the file modified" and for undo), and a mate added in one place moves nothing in
  another.
- **Which way round** two planes go (against each other, or the same way with `flip`)
  can't be told apart by equations that are well behaved at the solution, so a
  component that is the wrong way round is turned over before the solve.
- **Conflicts.** If a group's mates can't all hold, they are added one at a time in
  their order; each that can't be satisfied with those before it is flagged and left
  out, and the rest hold. A mate that can't be set up (a face that is gone, two ends on
  one component, a flat face for a concentric mate) is flagged the same way, with what
  to do. Redundant mates that agree are not flagged.
- **Freedom**: six for each component that is not fixed, less the rank of the
  Jacobian, for the assembly as a whole; and for each component, in how many
  independent ways it moves among the motions the mates leave: the rank of its six rows
  of a basis of the Jacobian's null space. A component that only moves along with
  others counts their motion as its own (two parts fastened to each other and to
  nothing else each have six), which is what answers "what is still loose". Each group
  is worked out on its own, and only when the mates change.
- The solver's structure and the freedom are kept between solves of the same mates, so
  a drag costs only the numeric solve: 50 components held by 147 mates take about 3 ms
  per step in a release build, against the 4 ms budget
  (`crates/peet-model/tests/mates.rs`).
- Operations `mate`, `edit_mate`, `mates`, and `rename`, `suppress`, `delete` with a
  `mate`. A script describes an end with the selectors a part's operations take, in the
  part's own coordinates. Replies list the components an operation moved. In the
  application: a click in an assembly picks a face, an edge or a corner (and with it
  its component), six mate commands that work on two picked ends, the mates in the
  tree, and a mate's properties.

- **Dragging.** A drag pulls a point of a component towards a place (`peet_model::Drag`,
  the `drag` operation, the left mouse button on a component in the view). The
  component's group is moved in steps, as the sketcher drags: the least squares answer
  to the mates' equations, linearised, with weak equations that pull the point; then
  the mates solved exactly. A pull is taken a reach (10 mm) at a time, so that a far or
  unreachable place doesn't bend the mates in the step it asks for, and a step that
  overshoots round a curve is halved. It is done first without letting the dragged
  component turn, then letting it: a part that can slide to the place slides, a hinged
  one swings. In the view the point is the one under the pointer when the button went
  down, and it is pulled in the plane through it that faces the viewer; the steps of
  one drag are one undo step. Pulling a stack of 50 plates held by 147 mates by a
  corner takes about 3 ms a step in a release build (7 ms for the first, which sets the
  solver up).

Left of stage 3: a script can't describe a face of a part inside a
sub-assembly (the application can pick one, and the model and the solve handle it);
tangent and other mates beyond the roadmap's list. Mates were added to the same model schema,
which no released version writes, without another version number.

## What stage 4 built

- **Interference** (`Document::interferences`, the `interference` query, the
  Interference Check window). Every two bodies of different components whose boxes
  overlap are placed and intersected by the kernel's boolean; a result with volume is an
  interference, reported with its volume and box. The boolean treats faces on one
  surface (faces against each other, a pin in a hole of its size) as touching, so mated
  parts are not reported. A pair the boolean can't handle is listed as not compared,
  never as clear. Nothing is cached: an assembly is checked when asked.
- **Bill of materials** (`Document::bill_of_materials`, `bom`, `export` to CSV, the
  window). One line per part with how many: the same part counts as one wherever it is
  used, through sub-assemblies (or a sub-assembly is one line). A line has the part's
  material, volume and mass, and for a sheet metal part its thickness, flat size and
  number of bends, taken from the part's own sheet definition.
- **Mass** (`Document::assembly_mass`, `mass`, the Mass Properties window). Each body
  is measured once (instances share the measurement), placed, and weighed with its own
  part's density; the centre of gravity and the principal moments are of the mass. If a
  part has no material the mass is not given, the parts without one are named, and the
  centre and moments are those of the volume.

## What stage 5 built

- **Visibility.** A hidden component (`Component::visible`, there since stage 2) is
  left out of what is drawn, picked and framed by zoom to fit, and of nothing else.
  `show_all` and `isolate` change several at once, as one undo step each.
- **Colour.** `Component::color` overrides the part's colour for one instance; a
  sub-assembly's goes for everything in it. Set by `set_color` with `component`.
- **Exploded view.** `Assembly` holds `ExplodeStep`s: a name, components of the
  assembly, and an offset in the assembly's coordinates. They are model data (saved,
  undone, translated to operations: `explode_step`, `edit_explode_step`). A component
  in several steps moves by their sum; a deleted component leaves its steps, and an
  empty step is dropped. Steps move only the assembly's own components: a sub-assembly
  is one thing.
- **Shown exploded is a view.** `Document::set_explode(amount)` (0 to 1) moves only
  `Placed::shown`, the frame a body is drawn and picked at; `Placed::frame`, which
  mates, interference, mass, bounds and exports use, never changes. So an exploded
  view can't affect a mate, and nothing is rebuilt to show one: the application
  animates `amount` over 0.4 s, moving instance transforms only. Dragging a component
  is refused while the view is exploded.
- Component and explode fields were added to the same, still unreleased, model schema, so an
  assembly file written by an earlier build of this phase does not open.

## What stage 6 built

- **Instanced drawing.** The renderer groups a frame's objects by mesh and draws each
  group in one call, with a per-instance transform, tint and pick id as instance
  vertex attributes (`peet-render/src/batch.rs`); the per-object uniform buffer is
  gone. Before this the transform lived with the GPU mesh, so the instances of a part,
  which share a mesh, were all drawn where the last of them was: stage 2's "shared
  meshes" drew wrongly, and no test drew anything. `peet-render/tests/gpu.rs` now draws
  on a real adapter (skipped where there is none) and picks two objects of one mesh
  each in its own place.
- **Culling.** An object whose box is wholly beyond one side of the view is left out
  before anything reaches the GPU (`peet_render::in_view`), in the picture and in the
  pick pass, whose view is the few pixels round the cursor.
- **On demand.** A body is tessellated and its mesh uploaded when it is first drawn,
  not when the document changes: what is hidden or never comes into view costs
  nothing. An assembly's file keeps the display meshes of the bodies that were drawn
  (by body stamp, with the model's hash); when it is opened its parts are still
  rebuilt, and a body whose stamp is in the file takes its mesh from there instead of
  being tessellated. Nothing is tessellated just to be saved, so the command line,
  which draws nothing, writes none and keeps the ones a file has.
- **Level of detail** (`peet-ui/src/lod.rs`). A body drawn under 100 pixels across
  gets a coarse mesh (`tessellate_with`: facets up to 30° of arc instead of 10°, the
  same faces and edges, so picking is unchanged), made and uploaded the first time it
  is needed; under 28 pixels it has no edges or silhouette lines. Silhouettes of
  bodies of one part turned the same way are worked out once in a parallel view.
- Measured: 1,000 instances of 50 parts (1.9 million triangles, a million edge
  segments, 1600 × 1000 with 4× MSAA and picking) take 102 draw calls and about 1 ms a
  frame on an RTX 4070 Super, drawn and waited for; deciding what to draw takes under
  1 ms for 1,000 bodies in an unoptimised build.

## Component patterns and the exit assembly

- **A pattern's copies are components.** `ComponentPattern` (in `Assembly`) has its
  originals, a `PatternKind` (linear in one or two directions, or circular) and its
  instances: for each original and each place, the component that is the copy. The
  copies are made and removed when the pattern is added or changed (`sync_pattern`),
  so everything that works on components (the tree, the bill of materials, mass,
  interference, STEP, visibility, explode steps) works on them with no code of its own.
  The alternative, instances that exist only in the evaluation, would have needed each
  of those to learn about patterns.
- **A copy is placed after the mates are solved.** The solver treats a copy as fixed;
  once the mates are solved, each pattern works out its moves (a translation per
  place, or a rotation about the axis) and puts each copy where its original now is,
  moved. A direction or axis is fixed in the assembly or is geometry of a component
  (resolved by the mates' own code, `mate::locate`), so a pattern follows a component
  that moves or a part that changes. Because a copy's place is only known after the
  solve, a mate onto a copy would see it a rebuild late: mating, placing, dragging,
  fixing, replacing and deleting a copy are refused, with the pattern and the original
  named.
- **The exit assembly** (`samples::enclosure_assembly`, **Open Sample Assembly**,
  `open_sample` with `assembly`): the chassis (fixed), a cover on its rim flush with
  the right and back walls, the housing on the cover over an opening for its shaft,
  an M8 socket screw in a counterbore patterned round the housing's bore (six), and an
  M4 screw in a mounting hole patterned along the walls (four). Twelve mates; a
  parallel mate on a flat of each screw's socket stops it spinning, so nothing is
  left free. The housing sits on the cover because the chassis's floor is covered in
  louvers and dimples: there is no flat 100 mm circle on it.
- `crates/peet-ops/tests/enclosure_exit.rs` checks the criteria: every mate holds and
  the freedom is 0; no interference (screws in their holes and heads on their seats
  only touch); the bill of materials is 1 + 1 + 1 + 6 + 4 with masses from the sizes
  (the cover, the housing by Pappus, the screws as two cylinders less a hexagon
  socket; the chassis from its own measured volume) and flat sizes (the chassis's
  333.572 × 215.876 blank, the cover's 240 × 160); and making the chassis 40 wider
  moves the cover, the housing and its six bolts 40 with the wall they hang on, while
  the screws stay in the mounting holes, which are measured from the left.
- The chassis sample's rim profile is now sketched at the left corner of the base
  instead of the right: at the right it was at a fixed position, so the part could
  not be made wider. Its solid, flat pattern and STEP file are unchanged (the Phase 5
  tests pass as they were).

## Consequences

- A machine whose graphics can't draw instanced geometry can't run the viewport (WebGL2
  and every wgpu backend can).
- An assembly file grows with the parts in it, and a part used by two assemblies is two
  copies until linking exists.
- A part can't be shaped by its neighbours (a hole placed from the mating part). It has
  to be dimensioned by shared parameter values kept in step by hand.
- A mechanism inside a sub-assembly doesn't move in its parent.
- Moving the solver's core is a refactoring of the most heavily tested numerical code in
  the project; its tests and benchmark must pass unchanged, and the drag benchmark must
  not slow down.
- Operations that mean "the document" now mean "the current document", and the hosts
  (the application, the command line, tests) hold a session.

## Merging with configurations

Configurations, sketch splines and the live session (`peet-live`) were merged to main
while this was on its branch. Both had given the model new schemas 5 and 6 with
different layouts. Main's numbers stand (5 and 6 for configurations, 7 for splines);
everything of this phase is model schema 8, and schema 6 and 7 files are read as
`ModelV7` (a model with configurations, without material, colour or assembly) and
converted. Files written by this branch before the merge are not read: no released
version wrote them. An assembly has no configurations of its own (each of its parts has
its own), so the configuration operations and the configuration list are refused or
hidden in an assembly, as the other part operations are.
